//! Why a Rust function is called from outside the workspace's own code.
//!
//! Rust has no single "export" like JavaScript. Something outside the code
//! calls a function when it is:
//!
//! - `main`: the binary's start (`fn main` at a crate root);
//! - `test` / `bench`: run by `cargo test` or `cargo bench`;
//! - `export`: public API of a library crate (`pub fn` reachable through
//!   `pub mod`s, or a `pub` method of a public type);
//! - `ffi`: called by name from C (`#[no_mangle]`, `#[export_name]`);
//! - `trait-impl`: a method implementing a trait from outside the
//!   workspace (`Display::fmt`, `Drop::drop`, `Iterator::next`), which
//!   library code calls.

use language_core::EntryPoint;
use ra_ap_hir::{
    AsAssocItem, AssocItemContainer, Crate, Function, HasVisibility, Module, ModuleDef, Visibility,
};
use ra_ap_ide_db::RootDatabase;

fn entry(kind: &str, label: String) -> EntryPoint {
    EntryPoint {
        kind: kind.to_string(),
        label,
    }
}

fn crate_name(db: &RootDatabase, krate: Crate) -> String {
    krate
        .display_name(db)
        .map(|name| name.to_string())
        .unwrap_or_else(|| "crate".to_string())
}

/// A crate with `fn main` at its root is a binary; its `pub` items are not
/// anyone's API.
fn is_binary(db: &RootDatabase, krate: Crate) -> bool {
    krate
        .root_module(db)
        .declarations(db)
        .into_iter()
        .any(|def| matches!(def, ModuleDef::Function(f) if f.is_main(db)))
}

/// `pub` all the way from the crate root down to `module`.
fn public_path(db: &RootDatabase, module: Module) -> bool {
    module
        .path_to_root(db)
        .into_iter()
        .filter(|m| !m.is_crate_root(db))
        .all(|m| m.visibility(db) == Visibility::Public)
}

/// `crate::a::b::name`, with the crate's own name.
fn item_path(db: &RootDatabase, module: Module, name: &str) -> String {
    let mut parts = vec![crate_name(db, module.krate(db))];
    parts.extend(
        module
            .path_to_root(db)
            .into_iter()
            .rev()
            .filter_map(|m| m.name(db).map(|n| n.as_str().to_string())),
    );
    parts.push(name.to_string());
    parts.join("::")
}

/// Why `function` is an entry point, from rust-analyzer's view of it and
/// the attributes written on it.
pub(crate) fn entry_points(
    db: &RootDatabase,
    function: Function,
    attrs: &[String],
) -> Vec<EntryPoint> {
    let mut out = Vec::new();
    let name = function.name(db).as_str().to_string();
    let module = function.module(db);
    let krate = module.krate(db);
    let path = item_path(db, module, &name);
    // `#[tokio::test]` and other test macros are not expanded (proc macros
    // are off), so their names count too.
    let attr_named = |wanted: &str| {
        attrs.iter().any(|attr| {
            let path = attr.split(['(', '=', ' ']).next().unwrap_or("");
            path == wanted || path.ends_with(&format!("::{wanted}"))
        })
    };
    if function.is_main(db) {
        out.push(entry("main", format!("{} main", crate_name(db, krate))));
    }
    if function.is_test(db) || attr_named("test") {
        out.push(entry("test", path.clone()));
    }
    if function.is_bench(db) || attr_named("bench") {
        out.push(entry("bench", path.clone()));
    }
    let ffi = attrs.iter().any(|attr| {
        let attr = attr.trim_start_matches("unsafe(");
        attr.starts_with("no_mangle") || attr.starts_with("export_name")
    });
    if ffi && !function.is_main(db) {
        out.push(entry("ffi", name.clone()));
    }
    match function.as_assoc_item(db).map(|item| item.container(db)) {
        Some(AssocItemContainer::Impl(implementation)) => match implementation.trait_(db) {
            Some(tr) => {
                if !tr.module(db).krate(db).origin(db).is_local() {
                    let ty = implementation
                        .self_ty(db)
                        .as_adt()
                        .map(|adt| adt.name(db).as_str().to_string());
                    let label = match ty {
                        Some(ty) => format!("{}::{name} for {ty}", tr.name(db).as_str()),
                        None => format!("{}::{name}", tr.name(db).as_str()),
                    };
                    out.push(entry("trait-impl", label));
                }
            }
            None => {
                // An inherent method: API when it and its type are public.
                let adt = implementation.self_ty(db).as_adt();
                if let Some(adt) = adt {
                    if !is_binary(db, krate)
                        && function.visibility(db) == Visibility::Public
                        && adt.visibility(db) == Visibility::Public
                        && public_path(db, adt.module(db))
                    {
                        let owner = adt.name(db).as_str().to_string();
                        out.push(entry(
                            "export",
                            item_path(db, adt.module(db), &owner) + "::" + &name,
                        ));
                    }
                }
            }
        },
        Some(AssocItemContainer::Trait(tr)) => {
            // A provided method of a public trait.
            if !is_binary(db, krate)
                && tr.visibility(db) == Visibility::Public
                && public_path(db, tr.module(db))
            {
                let owner = tr.name(db).as_str().to_string();
                out.push(entry(
                    "export",
                    item_path(db, tr.module(db), &owner) + "::" + &name,
                ));
            }
        }
        None => {
            if !is_binary(db, krate)
                && function.visibility(db) == Visibility::Public
                && public_path(db, module)
            {
                out.push(entry("export", path));
            }
        }
    }
    out
}

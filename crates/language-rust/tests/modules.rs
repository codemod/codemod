//! symbols(rust) module functions: a module path to its file, a file back
//! to the path that reaches it, and who brings a module in.

mod support;
use support::Project;

fn shop() -> Project {
    Project::new(&[
        ("src/lib.rs", "pub mod billing;\nmod report;\n"),
        ("src/billing/mod.rs", "pub mod invoice;\n"),
        ("src/billing/invoice.rs", "pub fn issue() {}\n"),
        (
            "src/report.rs",
            "use crate::billing::invoice;\npub fn run() { invoice::issue() }\n",
        ),
    ])
}

#[test]
fn a_crate_path_resolves_to_the_module_file() {
    let shop = shop();
    use language_core::SemanticProvider;
    let found = shop
        .provider
        .resolve_module(&shop.path("src/report.rs"), "crate::billing::invoice")
        .unwrap();
    assert_eq!(found, Some(shop.path("src/billing/invoice.rs")));
}

#[test]
fn a_super_path_resolves_from_the_file_s_module() {
    let shop = shop();
    use language_core::SemanticProvider;
    let found = shop
        .provider
        .resolve_module(&shop.path("src/billing/invoice.rs"), "super")
        .unwrap();
    assert_eq!(found, Some(shop.path("src/billing/mod.rs")));
}

#[test]
fn a_file_is_reached_by_its_crate_path() {
    let shop = shop();
    use language_core::SemanticProvider;
    let found = shop
        .provider
        .specifier_for(
            &shop.path("src/report.rs"),
            &shop.path("src/billing/invoice.rs"),
            &Default::default(),
        )
        .unwrap();
    assert_eq!(found.as_deref(), Some("crate::billing::invoice"));
}

#[test]
fn importers_are_the_mod_declaration_and_the_use_items() {
    let shop = shop();
    use language_core::SemanticProvider;
    let found = shop
        .provider
        .importers(&shop.path("src/billing/invoice.rs"))
        .unwrap();
    let mut importers: Vec<String> = found
        .files
        .iter()
        .flat_map(|f| {
            let file = f
                .file_path
                .strip_prefix(shop.path(""))
                .unwrap()
                .display()
                .to_string();
            f.importers
                .iter()
                .map(move |i| format!("{file}: {}", i.specifier))
        })
        .collect();
    importers.sort();
    assert_eq!(
        importers,
        [
            "src/billing/mod.rs: mod invoice",
            "src/report.rs: crate::billing::invoice"
        ]
    );
}

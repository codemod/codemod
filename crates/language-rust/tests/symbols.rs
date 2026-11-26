//! symbols(rust) and types(rust): where a name is declared, every place
//! it is used, and what type rust-analyzer infers for a value.

mod support;
use support::Project;

#[test]
fn a_name_imported_with_use_is_declared_in_its_module() {
    let shop = Project::new(&[
        (
            "src/lib.rs",
            "mod tax;\nuse tax::vat;\npub fn total(net: u32) -> u32 { net + vat(net) }\n",
        ),
        ("src/tax.rs", "pub fn vat(net: u32) -> u32 { net / 5 }\n"),
    ]);
    assert_eq!(
        shop.definition("src/lib.rs", "vat(net)").as_deref(),
        Some("src/tax.rs:vat")
    );
}

#[test]
fn a_method_call_is_declared_in_its_impl() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub struct Cart;\nimpl Cart { pub fn sum(&self) -> u32 { 0 } }\npub fn f(c: Cart) -> u32 { c.sum() }\n",
    )]);
    let declared = shop.definition("src/lib.rs", "sum()").unwrap();
    assert_eq!(declared, "src/lib.rs:sum");
    assert_eq!(
        shop.range("src/lib.rs", "sum"),
        shop.range("src/lib.rs", "sum#1")
    );
}

#[test]
fn a_local_is_declared_at_its_let() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub fn f() -> u32 { let total = 1; total + 1 }\n",
    )]);
    assert_eq!(
        shop.definition("src/lib.rs", "total#2").as_deref(),
        Some("src/lib.rs:total")
    );
}

#[test]
fn references_cover_every_file_of_the_crate() {
    let shop = Project::new(&[
        (
            "src/lib.rs",
            "mod tax;\nmod order;\npub fn report() -> u32 { tax::vat(1) }\n",
        ),
        ("src/tax.rs", "pub fn vat(net: u32) -> u32 { net / 5 }\n"),
        (
            "src/order.rs",
            "use crate::tax::vat;\npub fn total() -> u32 { vat(2) }\n",
        ),
    ]);
    assert_eq!(
        shop.references("src/tax.rs", "vat"),
        ["src/lib.rs:vat", "src/order.rs:vat", "src/order.rs:vat"]
    );
}

#[test]
fn a_let_without_a_type_has_its_inferred_type() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub fn names() -> usize { let all = vec![String::new()]; all.len() }\n",
    )]);
    assert_eq!(
        shop.type_of("src/lib.rs", "all").as_deref(),
        Some("Vec<String>")
    );
}

#[test]
fn a_method_call_has_the_type_it_returns() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub fn first(xs: &[u32]) -> u32 { let x = xs.first(); x.copied().unwrap_or(0) }\n",
    )]);
    assert_eq!(
        shop.type_of("src/lib.rs", "xs.first()").as_deref(),
        Some("Option<&u32>")
    );
}

#[test]
fn a_function_has_its_signature_as_type() {
    let shop = Project::new(&[("src/lib.rs", "pub fn vat(net: u32) -> u32 { net / 5 }\n")]);
    assert_eq!(
        shop.type_of("src/lib.rs", "vat").as_deref(),
        Some("pub fn vat(net: u32) -> u32")
    );
}

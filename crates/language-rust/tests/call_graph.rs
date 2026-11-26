//! callGraph(rust): which functions a function calls, and who calls it.
//! rust-analyzer resolves each call, so paths, methods and trait dispatch
//! all land on the function that runs.

mod support;
use support::{has, Project};

#[test]
fn a_call_through_use_lands_in_the_other_file() {
    let shop = Project::new(&[
        (
            "src/lib.rs",
            "mod tax;\nuse tax::vat;\npub fn total(net: u32) -> u32 { net + vat(net) }\n",
        ),
        ("src/tax.rs", "pub fn vat(net: u32) -> u32 { net / 5 }\n"),
    ]);
    assert_eq!(
        shop.callees("src/lib.rs", "total"),
        ["src/tax.rs:pub fn vat(net: u32) -> u32 (direct)"]
    );
}

#[test]
fn a_method_call_lands_on_the_impl_method() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub struct Cart { items: Vec<u32> }\n\
         impl Cart {\n    pub fn sum(&self) -> u32 { self.items.iter().sum() }\n}\n\
         pub fn checkout(cart: &Cart) -> u32 { cart.sum() }\n",
    )]);
    assert_eq!(
        shop.callees("src/lib.rs", "checkout"),
        ["src/lib.rs:pub fn sum(&self) -> u32 (method)"]
    );
}

#[test]
fn a_trait_method_on_dyn_reaches_every_impl_in_the_workspace() {
    let shop = Project::new(&[
        (
            "src/lib.rs",
            "mod card;\nmod cash;\n\
             pub trait Pay { fn pay(&self) -> u32; }\n\
             pub fn settle(method: &dyn Pay) -> u32 { method.pay() }\n",
        ),
        (
            "src/card.rs",
            "pub struct Card;\nimpl crate::Pay for Card { fn pay(&self) -> u32 { 1 } }\n",
        ),
        (
            "src/cash.rs",
            "pub struct Cash;\nimpl crate::Pay for Cash { fn pay(&self) -> u32 { 2 } }\n",
        ),
    ]);
    assert_eq!(
        shop.callees("src/lib.rs", "settle"),
        [
            "src/card.rs:fn pay(&self) -> u32 (type)",
            "src/cash.rs:fn pay(&self) -> u32 (type)"
        ]
    );
}

#[test]
fn a_call_into_std_is_unresolved() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub fn size(name: &str) -> usize { name.len() }\n",
    )]);
    assert_eq!(
        shop.callees("src/lib.rs", "size"),
        ["src/lib.rs:name.len() (unresolved)"]
    );
}

#[test]
fn callers_come_from_every_file_of_the_crate() {
    let shop = Project::new(&[
        ("src/lib.rs", "mod tax;\nmod order;\n"),
        ("src/tax.rs", "pub fn vat(net: u32) -> u32 { net / 5 }\n"),
        (
            "src/order.rs",
            "use crate::tax;\npub fn total(net: u32) -> u32 { net + tax::vat(net) }\n",
        ),
    ]);
    assert_eq!(
        shop.callers("src/tax.rs", "vat"),
        ["src/order.rs:pub fn total(net: u32) -> u32 (direct)"]
    );
}

#[test]
fn a_function_passed_as_a_value_is_called_through_it() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "fn double(x: u32) -> u32 { x * 2 }\n\
         pub fn run() -> u32 { let f = double; f(21) }\n",
    )]);
    assert!(has(
        &shop.callees("src/lib.rs", "run"),
        "fn double(x: u32) -> u32"
    ));
}

#[test]
fn a_constructor_is_not_a_call() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub struct Id(u32);\npub fn make() -> Option<Id> { Some(Id(1)) }\n",
    )]);
    assert!(shop.callees("src/lib.rs", "make").is_empty());
}

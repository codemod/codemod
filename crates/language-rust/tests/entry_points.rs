//! entryPoints(rust): functions that something outside the workspace's
//! own code calls, so having no callers does not make them dead.

mod support;
use support::Project;

const LIB: &str = "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

#[test]
fn main_of_a_binary_is_an_entry_point() {
    let shop = Project::new(&[("src/main.rs", "fn main() {}\n")]);
    assert_eq!(shop.entry_points(), ["main shop main"]);
}

#[test]
fn a_pub_fn_of_a_library_is_an_export() {
    let shop = Project::new(&[
        ("Cargo.toml", LIB),
        ("src/lib.rs", "pub mod tax {\n    pub fn vat() {}\n}\n"),
    ]);
    assert_eq!(shop.entry_points(), ["export shop::tax::vat"]);
}

#[test]
fn a_pub_fn_inside_a_private_module_is_not_an_export() {
    let shop = Project::new(&[("src/lib.rs", "mod tax {\n    pub fn vat() {}\n}\n")]);
    assert!(shop.entry_points().is_empty());
}

#[test]
fn a_pub_method_of_a_pub_type_is_an_export() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub struct Cart;\nimpl Cart { pub fn sum(&self) {} }\n",
    )]);
    assert_eq!(shop.entry_points(), ["export shop::Cart::sum"]);
}

#[test]
fn a_test_is_an_entry_point() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds() {}\n}\n",
    )]);
    assert_eq!(shop.entry_points(), ["test shop::tests::adds"]);
}

#[test]
fn an_impl_of_a_std_trait_is_called_by_std() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "struct Money(u32);\nimpl std::fmt::Display for Money {\n    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { write!(f, \"{}\", self.0) }\n}\n",
    )]);
    assert_eq!(shop.entry_points(), ["trait-impl Display::fmt for Money"]);
}

#[test]
fn a_no_mangle_function_is_called_from_c() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "#[no_mangle]\nextern \"C\" fn shop_init() {}\n",
    )]);
    assert_eq!(shop.entry_points(), ["ffi shop_init"]);
}

//! dataflow(rust): where a value comes from (sources) and where it goes
//! (uses), through `let`s, patterns, fields, calls and macros.

mod support;
use support::{has, Project};

#[test]
fn a_let_comes_from_its_initializer() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub fn f(price: u32) -> u32 { let net = price * 2; net }\n",
    )]);
    assert!(has(
        &shop.sources("src/lib.rs", "net#2"),
        "src/lib.rs:price"
    ));
}

#[test]
fn a_parameter_comes_from_the_argument_in_another_file() {
    let shop = Project::new(&[
        (
            "src/lib.rs",
            "mod tax;\npub fn total() -> u32 { let base = 100; tax::vat(base) }\n",
        ),
        ("src/tax.rs", "pub fn vat(net: u32) -> u32 { net / 5 }\n"),
    ]);
    assert!(has(&shop.sources("src/tax.rs", "net /"), "src/lib.rs:100"));
}

#[test]
fn a_returned_value_flows_to_the_caller() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "fn secret() -> String { String::from(\"pw\") }\npub fn f() -> usize { let s = secret(); s.len() }\n",
    )]);
    assert!(has(
        &shop.uses("src/lib.rs", "String::from(\"pw\")"),
        "src/lib.rs:s.len"
    ));
}

#[test]
fn a_struct_field_comes_from_the_struct_literal() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub struct User { pub email: String }\n\
         pub fn f(input: String) -> String { let u = User { email: input }; u.email }\n",
    )]);
    assert!(has(
        &shop.sources("src/lib.rs", "u.email"),
        "src/lib.rs:input"
    ));
}

#[test]
fn a_destructured_tuple_part_comes_from_the_tuple() {
    // The shared walk does not track positions: y's sources include a as
    // well as b.
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub fn f(a: u32, b: u32) -> u32 { let (x, y) = (a, b); y }\n",
    )]);
    assert!(has(&shop.sources("src/lib.rs", "y#2"), "src/lib.rs:b"));
}

#[test]
fn an_if_let_binding_comes_from_the_matched_value() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub fn f(found: Option<u32>) -> u32 { if let Some(v) = found { v } else { 0 } }\n",
    )]);
    assert!(has(&shop.sources("src/lib.rs", "v }"), "src/lib.rs:found"));
}

#[test]
fn a_local_named_in_format_flows_into_it() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub fn f(token: String) -> String { let line = format!(\"t={}\", token); line }\n",
    )]);
    let uses = shop.uses("src/lib.rs", "token");
    assert!(has(&uses, "src/lib.rs:line"), "{uses:?}");
}

#[test]
fn a_closure_parameter_comes_from_what_the_closure_is_called_with() {
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub fn f(seed: u32) -> u32 { let add = |n: u32| n + 1; add(seed) }\n",
    )]);
    assert!(has(&shop.sources("src/lib.rs", "n + 1"), "src/lib.rs:seed"));
}

#[test]
fn a_call_to_a_mut_self_method_mutates_the_receiver() {
    use language_core::SemanticProvider;
    let shop = Project::new(&[(
        "src/lib.rs",
        "pub fn f() -> Vec<u32> { let mut seen = Vec::new(); seen.push(1); seen }\n",
    )]);
    let found = shop
        .provider
        .get_mutations(
            &shop.path("src/lib.rs"),
            shop.range("src/lib.rs", "seen"),
            Default::default(),
        )
        .unwrap();
    let kinds: Vec<String> = found
        .mutations
        .iter()
        .map(|m| format!("{} {}", m.kind.as_str(), m.confidence.as_str()))
        .collect();
    assert_eq!(kinds, ["method-call likely"]);
}

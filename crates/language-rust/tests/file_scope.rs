//! In file scope each file is read on its own, without its Cargo project.

use language_core::SemanticProvider;
use language_rust::RaSemanticProvider;

#[test]
fn a_file_on_its_own_still_resolves_its_own_names() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().canonicalize().unwrap().join("tool.rs");
    let text = "fn helper() -> u32 { 1 }\nfn main() { let n = helper(); println!(\"{n}\"); }\n";
    std::fs::write(&file, text).unwrap();
    let provider = RaSemanticProvider::file_scope();
    let at = text.find("helper()").unwrap() as u32;
    let found = provider
        .get_definition(
            &file,
            language_core::ByteRange::new(at, at + 6),
            Default::default(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        found.location.range.start as usize,
        text.find("helper").unwrap()
    );
}

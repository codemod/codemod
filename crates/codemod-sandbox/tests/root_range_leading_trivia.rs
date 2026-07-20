/// Regression test documenting the tree-sitter quirk that caused the
/// "false modified" bug: a JavaScript program's root node range excludes
/// leading trivia (e.g. a blank line before the first statement), so code
/// that reconstructs file content from the root node's own text/range
/// (rather than the full document source) silently drops that trivia.
///
/// `SgNodeRjs::source()` and `SgNodeRjs::commit_edits()` were fixed to use
/// the full document source instead of the root node's text; this test
/// pins down the underlying tree-sitter behavior they now work around.
#[test]
fn root_range_excludes_leading_newline() {
    use ast_grep_core::AstGrep;
    use ast_grep_language::SupportLang;

    let src = "\nvar x = 1;\n";
    let grep = AstGrep::new(src, SupportLang::JavaScript);
    let root = grep.root();

    assert_eq!(
        root.range(),
        1..12,
        "root node range should exclude the leading newline (byte 0)"
    );
    assert_ne!(
        root.text(),
        src,
        "root node text should differ from the full source because of the excluded leading newline"
    );
}

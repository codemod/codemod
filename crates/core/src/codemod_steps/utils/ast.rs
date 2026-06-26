use std::str::FromStr;

use ast_grep_core::tree_sitter::{LanguageExt, StrDoc};
use ast_grep_core::Node;
use codemod_sandbox::{ast_grep_parse_lock, CodemodLang};

pub(crate) type AstDoc = StrDoc<CodemodLang>;
pub(crate) type AstNode<'a> = Node<'a, AstDoc>;

pub(crate) fn ast_grep_root(
    content: &str,
    language: &str,
) -> Result<ast_grep_core::AstGrep<AstDoc>, String> {
    // Dynamic tree-sitter languages are registered in process-global state by ast-grep.
    // Serializing AST construction avoids racing parser initialization/use across test
    // and workflow task threads, which can otherwise surface as native crashes.
    let _guard = ast_grep_parse_lock()
        .lock()
        .map_err(|_| "ast-grep parser lock was poisoned".to_string())?;
    let language = CodemodLang::from_str(language)?;
    Ok(language.ast_grep(content))
}

pub(crate) fn nearest_ancestor<'a>(node: &AstNode<'a>, kind: &str) -> Option<AstNode<'a>> {
    node.ancestors().find(|ancestor| ancestor.kind() == kind)
}

pub(crate) fn node_text_starts_with(node: &AstNode<'_>, prefix: &str) -> bool {
    node.text().trim_start().starts_with(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ast_and_finds_nearest_ancestor() {
        let root = ast_grep_root(
            r#"
dependencies {
    implementation("org.slf4j:slf4j-api:2.0.9")
}
"#,
            "kotlin",
        )
        .unwrap();

        let string_literal = root
            .root()
            .dfs()
            .find(|node| node.kind() == "string_literal")
            .unwrap();
        let call = nearest_ancestor(&string_literal, "call_expression").unwrap();

        assert!(node_text_starts_with(&call, "implementation"));
    }

    #[test]
    fn reports_unsupported_language() {
        let error = match ast_grep_root("value", "not-a-language") {
            Ok(_) => panic!("expected unsupported language error"),
            Err(error) => error,
        };

        assert!(error.contains("Unsupported language"));
    }
}

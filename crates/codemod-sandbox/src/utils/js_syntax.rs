//! Syntax diagnostics for JavaScript and TypeScript input to JSSG.

use std::path::Path;

use ast_grep_language::SupportLang;
use oxc::{allocator::Allocator, parser::Parser, span::SourceType};

use crate::CodemodLang;

pub fn parse_error(language: CodemodLang, source: &str) -> Option<String> {
    // The JS ast-grep grammar also accepts JSX. Likewise, honor a step
    // explicitly selecting TS or TSX regardless of the input's suffix.
    let filename = match language {
        CodemodLang::Static(SupportLang::JavaScript) => "input.jsx",
        CodemodLang::Static(SupportLang::TypeScript) => "input.ts",
        CodemodLang::Static(SupportLang::Tsx) => "input.tsx",
        _ => return None,
    };
    let source_type = SourceType::from_path(Path::new(filename)).ok()?;
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    parsed.errors.first().map(ToString::to_string)
}

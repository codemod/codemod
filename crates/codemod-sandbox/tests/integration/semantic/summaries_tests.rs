//! Values followed across calls with per-function summaries
//! (`dataflow(lang, { interprocedural: true })`): one small fixture per
//! language, run in file and workspace scope. The cases are named in the
//! codemods.

use ast_grep_language::SupportLang;
use codemod_sandbox::CodemodLang;

jssg_test! {
    name: test_typescript_summaries_workspace,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "summaries_ts.js",
    fixture_dir: "typescript/summaries",
    target: "app.ts",
    scope: workspace,
}

jssg_test! {
    name: test_typescript_summaries_file,
    language: CodemodLang::Static(SupportLang::TypeScript),
    codemod: "summaries_ts.js",
    fixture_dir: "typescript/summaries",
    target: "app.ts",
}

jssg_test! {
    name: test_python_summaries_workspace,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "summaries_py.js",
    fixture_dir: "python/summaries",
    target: "app.py",
    scope: workspace,
}

jssg_test! {
    name: test_python_summaries_file,
    language: CodemodLang::Static(SupportLang::Python),
    codemod: "summaries_py.js",
    fixture_dir: "python/summaries",
    target: "app.py",
}

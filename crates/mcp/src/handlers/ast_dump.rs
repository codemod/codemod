use ast_grep_core::AstGrep;
use codemod_sandbox::CodemodLang;
use rmcp::{handler::server::wrapper::Parameters, model::*, schemars, tool, ErrorData as McpError};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DumpAstRequest {
    /// The source code to analyze
    pub source_code: String,
    /// The programming language (e.g., "javascript", "typescript", "python", "rust", etc.)
    pub language: String,
}

#[derive(Clone)]
pub struct AstDumpHandler;

impl AstDumpHandler {
    pub fn new() -> Self {
        Self
    }

    pub fn dump_ast_text(&self, source_code: &str, language: &str) -> Result<String, String> {
        let language = language
            .parse()
            .map_err(|error| format!("Unsupported language '{language}': {error}"))?;

        self.dump_ast_for_language(source_code, language)
            .map_err(|error| format!("Failed to parse AST: {error}"))
    }

    #[tool(
        description = "Dump AST nodes in an AI-friendly format for the given source code and language"
    )]
    pub async fn dump_ast(
        &self,
        Parameters(request): Parameters<DumpAstRequest>,
    ) -> Result<CallToolResult, McpError> {
        match self.dump_ast_text(&request.source_code, &request.language) {
            Ok(ast_dump) => Ok(CallToolResult::success(vec![Content::text(ast_dump)])),
            Err(e) => Err(McpError::invalid_params(e, None)),
        }
    }

    fn dump_ast_for_language(
        &self,
        source_code: &str,
        language: CodemodLang,
    ) -> Result<String, String> {
        let root = AstGrep::new(source_code, language);
        let result = self.dump_ast_for_ai_context(root.root(), 0);
        let formatted_result = format!("```\n{result}\n```");
        Ok(formatted_result)
    }

    #[allow(clippy::only_used_in_recursion)]
    fn dump_ast_for_ai_context<D>(&self, node: ast_grep_core::Node<D>, indent: usize) -> String
    where
        D: ast_grep_core::Doc,
    {
        let indent_str = " ".repeat(indent);
        let kind = node.kind();

        let mut result = format!("{indent_str}{kind}\n");

        for child in node.children() {
            result.push_str(&self.dump_ast_for_ai_context(child, indent + 1));
        }

        result
    }
}

impl Default for AstDumpHandler {
    fn default() -> Self {
        Self::new()
    }
}

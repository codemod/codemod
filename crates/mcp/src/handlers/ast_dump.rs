use ast_grep_core::AstGrep;
use ast_grep_language::SupportLang;
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

    #[tool(
        description = "Dump AST nodes in an AI-friendly format for the given source code and language"
    )]
    pub async fn dump_ast(
        &self,
        Parameters(request): Parameters<DumpAstRequest>,
    ) -> Result<CallToolResult, McpError> {
        let language = match request.language.parse() {
            Ok(lang) => lang,
            Err(e) => {
                return Err(McpError::invalid_params(
                    format!("Unsupported language '{}': {}", request.language, e),
                    None,
                ));
            }
        };

        match self.dump_ast_for_language(&request.source_code, language) {
            Ok(ast_dump) => Ok(CallToolResult::success(vec![Content::text(ast_dump)])),
            Err(e) => Err(McpError::internal_error(
                format!("Failed to parse AST: {e}"),
                None,
            )),
        }
    }

    fn dump_ast_for_language(
        &self,
        source_code: &str,
        language: SupportLang,
    ) -> Result<String, String> {
        let root = AstGrep::new(source_code, language);
        Ok(self.dump_ast_for_ai_context(root.root(), 0))
    }

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

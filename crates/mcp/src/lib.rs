use rmcp::{
    handler::server::router::tool::ToolRouter, model::*, schemars, service::RequestContext, tool,
    tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler,
};
use serde_json::json;

mod handlers;
use handlers::{AstDumpHandler, JssgTestHandler, NodeTypesHandler};

#[derive(Clone)]
pub struct CodemodMcpServer {
    ast_dump_handler: AstDumpHandler,
    node_types_handler: NodeTypesHandler,
    jssg_test_handler: JssgTestHandler,
    tool_router: ToolRouter<CodemodMcpServer>,
}

impl Default for CodemodMcpServer {
    fn default() -> Self {
        Self {
            ast_dump_handler: AstDumpHandler::new(),
            node_types_handler: NodeTypesHandler::new(),
            jssg_test_handler: JssgTestHandler::new(),
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl CodemodMcpServer {
    pub fn new() -> Self {
        Self::default()
    }

    fn _create_resource_text(&self, uri: &str, name: &str, description: Option<&str>) -> Resource {
        RawResource {
            uri: uri.to_string(),
            name: name.to_string(),
            description: description.map(|s| s.to_string()),
            mime_type: None,
            size: None,
            icons: None,
            title: None,
        }
        .no_annotation()
    }

    // Delegate to AST dump handler
    #[tool(
        description = "Dump AST nodes in an AI-friendly format for the given source code and language"
    )]
    async fn dump_ast(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<handlers::ast_dump::DumpAstRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.ast_dump_handler.dump_ast(params).await
    }

    // Delegate to node types handler
    #[tool(
        description = "Get compressed tree-sitter node types for a specific programming language in AI-friendly format"
    )]
    async fn get_node_types(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<
            handlers::node_types::GetNodeTypesRequest,
        >,
    ) -> Result<CallToolResult, McpError> {
        self.node_types_handler.get_node_types(params).await
    }

    #[tool(
        description = "Run tests for a jssg (JavaScript ast-grep) codemod with given test cases"
    )]
    async fn run_jssg_tests(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<handlers::jssg_test::RunJssgTestRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.jssg_test_handler.run_jssg_tests(params).await
    }

    #[tool(
        description = "Get jssg (JavaScript ast-grep) instructions for creating codemods (includes ast-grep fundamentals)"
    )]
    async fn get_jssg_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        let instructions_content = include_str!("data/prompts/jssg-instructions.md");
        Ok(CallToolResult::success(vec![Content::text(
            instructions_content,
        )]))
    }

    #[tool(
        description = "Get jssg-utils instructions for import manipulation helpers (getImport, addImport, removeImport)"
    )]
    async fn get_jssg_utils_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        let instructions_content = include_str!("data/prompts/jssg-utils-instructions.md");
        Ok(CallToolResult::success(vec![Content::text(
            instructions_content,
        )]))
    }

    #[tool(
        description = "Get Codemod CLI instructions for project setup and workflow configuration"
    )]
    async fn get_codemod_cli_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        let instructions_content = include_str!("data/prompts/codemod-cli-instructions.md");
        Ok(CallToolResult::success(vec![Content::text(
            instructions_content,
        )]))
    }
}

#[tool_handler]
impl ServerHandler for CodemodMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some("This server provides AST dumping, tree-sitter node types, and jssg (ast-grep with JS bindings) codemod testing tools. Available tools: dump_ast (get AI-friendly AST representation), get_node_types (get compressed tree-sitter node types), run_jssg_tests (run tests for jssg codemods), get_jssg_instructions (get jssg and ast-grep instructions), get_jssg_utils_instructions (get import manipulation helpers), get_codemod_cli_instructions (get CLI and workflow instructions). When you are asked to create a codemod or do a large refactor, you should use jssg and read both jssg-instructions (for writing codemods) and codemod-cli-instructions (for project setup). Use get_jssg_utils_instructions when you need to find, add, or remove imports.".to_string()),
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        tracing::info!("MCP server initialized");
        Ok(self.get_info())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult {
            resources: vec![
                self._create_resource_text(
                    "jssg://instructions",
                    "jssg-instructions",
                    Some(
                        "jssg instructions for creating codemods (includes ast-grep fundamentals)",
                    ),
                ),
                self._create_resource_text(
                    "jssg-utils://instructions",
                    "jssg-utils-instructions",
                    Some("jssg-utils instructions for import manipulation helpers (getImport, addImport, removeImport)"),
                ),
                self._create_resource_text(
                    "codemod-cli://instructions",
                    "codemod-cli-instructions",
                    Some("Codemod CLI instructions for project setup and workflow configuration"),
                ),
            ],
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        ReadResourceRequestParam { uri }: ReadResourceRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        match uri.as_str() {
            "jssg://instructions" => {
                let instructions_content = include_str!("data/prompts/jssg-instructions.md");
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(instructions_content, uri)],
                })
            }
            "jssg-utils://instructions" => {
                let instructions_content = include_str!("data/prompts/jssg-utils-instructions.md");
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(instructions_content, uri)],
                })
            }
            "codemod-cli://instructions" => {
                let instructions_content = include_str!("data/prompts/codemod-cli-instructions.md");
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(instructions_content, uri)],
                })
            }
            _ => Err(McpError::resource_not_found(
                "resource_not_found",
                Some(json!({
                    "uri": uri
                })),
            )),
        }
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult {
            next_cursor: None,
            resource_templates: Vec::new(),
        })
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetInstructionsRequest {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_server_creation() {
        let server = CodemodMcpServer::new();
        let info = server.get_info();

        assert_eq!(info.protocol_version, ProtocolVersion::V_2024_11_05);
        assert!(info.capabilities.tools.is_some());
        assert!(info.instructions.is_some());
    }
}

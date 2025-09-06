use rmcp::{
    handler::server::router::tool::ToolRouter, model::*, service::RequestContext, tool,
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

    fn _create_resource_text(&self, uri: &str, name: &str) -> Resource {
        RawResource::new(uri, name.to_string()).no_annotation()
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
        description = "Run tests for a JSSG (JavaScript AST-grep) codemod with given test cases"
    )]
    async fn run_jssg_tests(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<handlers::jssg_test::RunJssgTestRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.jssg_test_handler.run_jssg_tests(params).await
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
            instructions: Some("This server provides AST dumping, tree-sitter node types, JSSG codemod testing tools, and resources for various programming languages. Available tools: dump_ast (get AI-friendly AST representation), get_node_types (get compressed tree-sitter node types), run_jssg_tests (run tests for JSSG codemods). Available resources: jssg-instructions (instructions for creating JSSG codemods).".to_string()),
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
            resources: vec![self._create_resource_text("jssg://instructions", "jssg-instructions")],
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
                let instructions_content = include_str!("data/prompts/jssg-instructions.txt");
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

use rmcp::{
    handler::server::router::tool::ToolRouter, model::*, schemars, service::RequestContext, tool,
    tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler,
};
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod handlers;
use handlers::{
    AstDumpHandler, JssgTestHandler, KnowledgeHandler, NodeTypesHandler, PackageScaffoldHandler,
    PackageValidationHandler,
};

const PUBLIC_DOCS_TIMEOUT_SECS: u64 = 10;
const PUBLIC_DOCS_BUNDLE_TIMEOUT_SECS: u64 = 12;
const JSSG_INSTRUCTIONS: &str = include_str!("data/prompts/jssg-instructions.md");
const JSSG_UTILS_INSTRUCTIONS: &str = include_str!("data/prompts/jssg-utils-instructions.md");
const JSSG_RUNTIME_CAPABILITIES_INSTRUCTIONS: &str =
    include_str!("data/prompts/jssg-runtime-capabilities.md");
const CODEMOD_CLI_INSTRUCTIONS: &str = include_str!("data/prompts/codemod-cli-instructions.md");
const SHARDING_INSTRUCTIONS: &str = include_str!("data/prompts/sharding-instructions.md");

const CLI_DOC_URL: &str = "https://docs.codemod.com/cli.md";
const OSS_QUICKSTART_DOC_URL: &str = "https://docs.codemod.com/oss-quickstart.md";
const PACKAGE_STRUCTURE_DOC_URL: &str = "https://docs.codemod.com/package-structure.md";
const WORKFLOW_REFERENCE_DOC_URL: &str = "https://docs.codemod.com/workflows/reference.md";
const SHARDING_DOC_URL: &str = "https://docs.codemod.com/workflows/sharding.md";
const JSSG_QUICKSTART_DOC_URL: &str = "https://docs.codemod.com/jssg/quickstart.md";
const JSSG_REFERENCE_DOC_URL: &str = "https://docs.codemod.com/jssg/reference.md";
const JSSG_ADVANCED_DOC_URL: &str = "https://docs.codemod.com/jssg/advanced.md";
const JSSG_TESTING_DOC_URL: &str = "https://docs.codemod.com/jssg/testing.md";
const JSSG_UTILS_DOC_URL: &str = "https://docs.codemod.com/jssg/utils.md";
const JSSG_SECURITY_DOC_URL: &str = "https://docs.codemod.com/jssg/security.md";
const JSSG_SEMANTIC_ANALYSIS_DOC_URL: &str = "https://docs.codemod.com/jssg/semantic-analysis.md";
const CODEMOD_CREATION_WORKFLOW_INSTRUCTIONS: &str =
    include_str!("data/prompts/codemod-creation-workflow.md");

static PUBLIC_DOCS_CLIENT: LazyLock<Option<reqwest::Client>> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(PUBLIC_DOCS_TIMEOUT_SECS))
        .user_agent(format!("codemod-mcp/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()
});

fn strip_docs_index(doc: &str) -> &str {
    fn next_line_bounds(text: &str, start: usize) -> Option<(usize, usize)> {
        if start >= text.len() {
            return None;
        }

        let end = text[start..]
            .find('\n')
            .map(|offset| start + offset + 1)
            .unwrap_or(text.len());
        Some((start, end))
    }

    fn next_nonempty_line(text: &str, mut start: usize) -> Option<(usize, usize, &str)> {
        while let Some((line_start, line_end)) = next_line_bounds(text, start) {
            let line = text[line_start..line_end].trim_end_matches(['\r', '\n']);
            if !line.trim().is_empty() {
                return Some((line_start, line_end, line));
            }
            start = line_end;
        }

        None
    }

    let doc = doc.trim_start_matches('\u{feff}');
    let Some((mut line_start, mut line_end, mut line)) = next_nonempty_line(doc, 0) else {
        return doc;
    };

    if line == "---" {
        let mut cursor = line_end;
        while let Some((_, end, candidate)) = next_nonempty_line(doc, cursor) {
            cursor = end;
            if candidate == "---" {
                if let Some((start, end, candidate)) = next_nonempty_line(doc, cursor) {
                    line_start = start;
                    line_end = end;
                    line = candidate;
                } else {
                    return doc;
                }
                break;
            }
        }
    }

    if line.starts_with("# ") {
        return &doc[line_start..];
    }

    if line == "> ## Documentation Index" || line.starts_with("> ## Documentation Index") {
        let mut cursor = line_end;
        while let Some((start, end, candidate)) = next_nonempty_line(doc, cursor) {
            if candidate.starts_with('>') {
                cursor = end;
                continue;
            }

            return &doc[start..];
        }
    }

    doc
}

async fn fetch_public_doc_markdown(url: &str) -> Option<String> {
    let client = PUBLIC_DOCS_CLIENT.as_ref()?;
    let response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    let text = response.text().await.ok()?;
    Some(strip_docs_index(&text).trim().to_string())
}

async fn fetch_public_doc_sections(urls: &[&str]) -> Vec<String> {
    let fetches = urls.iter().copied().map(|url| async move {
        fetch_public_doc_markdown(url)
            .await
            .map(|content| format!("<!-- Source: {url} -->\n\n{content}"))
    });

    tokio::time::timeout(
        Duration::from_secs(PUBLIC_DOCS_BUNDLE_TIMEOUT_SECS),
        futures::future::join_all(fetches),
    )
    .await
    .unwrap_or_default()
    .into_iter()
    .flatten()
    .collect()
}

fn build_public_docs_bundle_from_sections(
    title: &str,
    sections: &[String],
    fallback: &str,
) -> String {
    if sections.is_empty() {
        return fallback.to_string();
    }

    format!(
        "# {title}\n\nThese instructions are sourced from the public Codemod docs deployment (`docs.codemod.com`). Prefer this content over older bundled examples when they differ.\n\n{}\n",
        sections.join("\n\n---\n\n")
    )
}

async fn build_public_docs_bundle(title: &str, urls: &[&str], fallback: &str) -> String {
    let sections = fetch_public_doc_sections(urls).await;
    build_public_docs_bundle_from_sections(title, &sections, fallback)
}

fn build_public_docs_bundle_with_supplement_from_sections(
    title: &str,
    sections: &[String],
    supplement_title: &str,
    supplement: &str,
    fallback: &str,
) -> String {
    if sections.is_empty() {
        return fallback.to_string();
    }

    format!(
        "# {title}\n\n# {supplement_title}\n\n{supplement}\n\n---\n\nThese instructions are sourced from the public Codemod docs deployment (`docs.codemod.com`). Prefer this content over older bundled examples when they differ.\n\n{}\n",
        sections.join("\n\n---\n\n")
    )
}

async fn build_public_docs_bundle_with_supplement(
    title: &str,
    urls: &[&str],
    supplement_title: &str,
    supplement: &str,
    fallback: &str,
) -> String {
    let sections = fetch_public_doc_sections(urls).await;
    build_public_docs_bundle_with_supplement_from_sections(
        title,
        &sections,
        supplement_title,
        supplement,
        fallback,
    )
}

#[derive(Clone)]
pub struct CodemodMcpServer {
    ast_dump_handler: AstDumpHandler,
    node_types_handler: NodeTypesHandler,
    jssg_test_handler: JssgTestHandler,
    knowledge_handler: KnowledgeHandler,
    package_scaffold_handler: PackageScaffoldHandler,
    package_validation_handler: PackageValidationHandler,
    usage_log_path: Option<PathBuf>,
    tool_router: ToolRouter<CodemodMcpServer>,
}

impl Default for CodemodMcpServer {
    fn default() -> Self {
        Self::new(None)
    }
}

impl CodemodMcpServer {
    pub fn new(usage_log_path: Option<PathBuf>) -> Self {
        Self {
            ast_dump_handler: AstDumpHandler::new(),
            node_types_handler: NodeTypesHandler::new(),
            jssg_test_handler: JssgTestHandler::new(),
            knowledge_handler: KnowledgeHandler::new(),
            package_scaffold_handler: PackageScaffoldHandler::new(),
            package_validation_handler: PackageValidationHandler::new(),
            usage_log_path,
            tool_router: Self::tool_router(),
        }
    }

    fn log_usage(&self, event: &str) {
        let Some(path) = self.usage_log_path.clone() else {
            return;
        };
        let event = event.to_string();
        let _handle = tokio::task::spawn_blocking(move || {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            if let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                if std::fs::create_dir_all(parent).is_err() {
                    return;
                }
            }
            let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
                return;
            };
            let _ = writeln!(file, "{timestamp}\t{event}");
        });
    }
}

#[tool_router]
impl CodemodMcpServer {
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
        self.log_usage("tool:dump_ast");
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
        self.log_usage("tool:get_node_types");
        self.node_types_handler.get_node_types(params).await
    }

    #[tool(
        description = "Run tests for a jssg (JavaScript ast-grep) codemod with given test cases"
    )]
    async fn run_jssg_tests(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<handlers::jssg_test::RunJssgTestRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:run_jssg_tests");
        self.jssg_test_handler.run_jssg_tests(params).await
    }

    #[tool(
        description = "Get the highest-priority verified JSSG gotchas before implementing a codemod transform."
    )]
    async fn get_jssg_gotchas(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<handlers::knowledge::KnowledgeListRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_jssg_gotchas");
        self.knowledge_handler.get_jssg_gotchas(params).await
    }

    #[tool(
        description = "Search the verified JSSG knowledge base for gotchas and recipes. Use this when implementing or repairing a codemod and you are unsure about a pattern or transform approach."
    )]
    async fn search_jssg_knowledge(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<
            handlers::knowledge::SearchKnowledgeRequest,
        >,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:search_jssg_knowledge");
        self.knowledge_handler.search_jssg_knowledge(params).await
    }

    #[tool(
        description = "Get the highest-priority verified ast-grep gotchas before implementing a codemod transform."
    )]
    async fn get_ast_grep_gotchas(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<handlers::knowledge::KnowledgeListRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_ast_grep_gotchas");
        self.knowledge_handler.get_ast_grep_gotchas(params).await
    }

    #[tool(
        description = "Search the verified ast-grep knowledge base for gotchas and recipes. Use this when a pattern is unclear or before considering regex or manual parsing fallbacks."
    )]
    async fn search_ast_grep_knowledge(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<
            handlers::knowledge::SearchKnowledgeRequest,
        >,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:search_ast_grep_knowledge");
        self.knowledge_handler.search_ast_grep_knowledge(params).await
    }

    #[tool(
        description = "Validate whether a codemod package in the current directory is real and complete. Use this before stopping work on a codemod package. It checks package surface completeness, workflow validity, test-case coverage, starter scaffold leftovers, and can run the package default tests and type-check script."
    )]
    async fn validate_codemod_package(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<
            handlers::package_validation::ValidateCodemodPackageRequest,
        >,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:validate_codemod_package");
        self.package_validation_handler
            .validate_codemod_package(params)
            .await
    }

    #[tool(
        description = "Scaffold a codemod package by delegating to the real `codemod init` CLI. Use this immediately after registry search shows there is no exact existing package for the requested migration."
    )]
    async fn scaffold_codemod_package(
        &self,
        params: rmcp::handler::server::wrapper::Parameters<
            handlers::package_scaffold::ScaffoldCodemodPackageRequest,
        >,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:scaffold_codemod_package");
        self.package_scaffold_handler
            .scaffold_codemod_package(params)
            .await
    }

    #[tool(
        description = "Get jssg (JavaScript ast-grep) instructions for creating codemods (includes ast-grep fundamentals)"
    )]
    async fn get_jssg_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_jssg_instructions");
        let instructions_content = build_public_docs_bundle(
            "Canonical JSSG Documentation",
            &[
                JSSG_QUICKSTART_DOC_URL,
                JSSG_REFERENCE_DOC_URL,
                JSSG_ADVANCED_DOC_URL,
                JSSG_TESTING_DOC_URL,
                JSSG_UTILS_DOC_URL,
                JSSG_SECURITY_DOC_URL,
                JSSG_SEMANTIC_ANALYSIS_DOC_URL,
            ],
            JSSG_INSTRUCTIONS,
        )
        .await;
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
        self.log_usage("tool:get_jssg_utils_instructions");
        let instructions_content = build_public_docs_bundle(
            "Canonical JSSG Import Utilities Documentation",
            &[JSSG_UTILS_DOC_URL],
            JSSG_UTILS_INSTRUCTIONS,
        )
        .await;
        Ok(CallToolResult::success(vec![Content::text(
            instructions_content,
        )]))
    }

    #[tool(
        description = "Get JSSG runtime and capability guidance for LLRT/Node APIs, codemod.yaml capabilities, and multi-file JSSG work"
    )]
    async fn get_jssg_runtime_capabilities(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_jssg_runtime_capabilities");
        Ok(CallToolResult::success(vec![Content::text(
            JSSG_RUNTIME_CAPABILITIES_INSTRUCTIONS,
        )]))
    }

    #[tool(
        description = "Get Codemod CLI instructions for project setup and workflow configuration"
    )]
    async fn get_codemod_cli_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_codemod_cli_instructions");
        let instructions_content = build_public_docs_bundle(
            "Canonical Codemod CLI and Workflow Documentation",
            &[
                CLI_DOC_URL,
                PACKAGE_STRUCTURE_DOC_URL,
                WORKFLOW_REFERENCE_DOC_URL,
                SHARDING_DOC_URL,
            ],
            CODEMOD_CLI_INSTRUCTIONS,
        )
        .await;
        Ok(CallToolResult::success(vec![Content::text(
            instructions_content,
        )]))
    }

    #[tool(
        description = "Get troubleshooting guidance for common Codemod CLI failures and unexpected behavior"
    )]
    async fn get_codemod_troubleshooting(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_codemod_troubleshooting");
        let instructions_content = include_str!("data/prompts/codemod-troubleshooting.md");
        Ok(CallToolResult::success(vec![Content::text(
            instructions_content,
        )]))
    }

    #[tool(
        description = "Get the codemod creation workflow guide for authoring, testing, and publishing codemods"
    )]
    async fn get_codemod_creation_workflow(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_codemod_creation_workflow");
        let instructions_content = build_public_docs_bundle_with_supplement(
            "Canonical Codemod Creation Documentation",
            &[
                OSS_QUICKSTART_DOC_URL,
                CLI_DOC_URL,
                PACKAGE_STRUCTURE_DOC_URL,
                WORKFLOW_REFERENCE_DOC_URL,
                JSSG_QUICKSTART_DOC_URL,
                JSSG_TESTING_DOC_URL,
            ],
            "Supplemental Local Guidance",
            CODEMOD_CREATION_WORKFLOW_INSTRUCTIONS,
            CODEMOD_CREATION_WORKFLOW_INSTRUCTIONS,
        )
        .await;
        Ok(CallToolResult::success(vec![Content::text(
            instructions_content,
        )]))
    }

    #[tool(
        description = "Get maintainer monorepo guide for setting up and managing codemod monorepos"
    )]
    async fn get_codemod_maintainer_monorepo(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_codemod_maintainer_monorepo");
        let instructions_content = include_str!("data/prompts/codemod-maintainer-monorepo.md");
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
            instructions: Some("This server provides AST dumping, tree-sitter node types, verified JSSG and ast-grep knowledge-base search, codemod package scaffolding and validation, and jssg (ast-grep with JS bindings) codemod testing tools. Available tools: dump_ast (get AI-friendly AST representation), get_node_types (get compressed tree-sitter node types), run_jssg_tests (run tests for jssg codemods), get_jssg_gotchas and search_jssg_knowledge (verified JSSG gotchas and recipes), get_ast_grep_gotchas and search_ast_grep_knowledge (verified ast-grep gotchas and recipes), scaffold_codemod_package (scaffold a codemod package via the real CLI), validate_codemod_package (validate a codemod package and detect starter/incomplete scaffolds plus risky transform strategies), get_codemod_troubleshooting (debug failing or unexpected codemod commands), get_codemod_creation_workflow (author, test, and publish codemods), get_codemod_maintainer_monorepo (set up and maintain a codemod monorepo), get_jssg_runtime_capabilities (runtime and capability guidance for LLRT/Node APIs and multi-file JSSG work). Available resources: jssg-instructions (public docs-backed JSSG guidance), jssg-utils-instructions (public docs-backed import utility guidance), jssg-runtime-capabilities-instructions (runtime and capability guidance), codemod-cli-instructions (public docs-backed CLI, package, and workflow guidance), sharding-instructions (public docs-backed sharding guidance), codemod-troubleshooting-instructions (Codemod CLI troubleshooting), codemod-creation-workflow-instructions (codemod creation workflow), codemod-maintainer-monorepo-instructions (maintainer monorepo guidance). When you are asked to create a codemod or do a large refactor, call get_jssg_gotchas and get_ast_grep_gotchas before writing source-transform code. If patterns are unclear, search the knowledge tools and use dump_ast before considering regex or manual parsing. If registry search finds no exact existing package, call scaffold_codemod_package immediately instead of stopping at analysis. Call validate_codemod_package before you stop work on a codemod package, and treat starter-scaffold findings or risky transform findings as blocking. Use get_jssg_runtime_capabilities when the codemod needs Node/LLRT APIs, gated modules like fs/fetch/child_process, or non-trivial multi-file JSSG work. Use get_jssg_utils_instructions when you need to find, add, or remove imports. Use sharding-instructions when you need to split a large migration into multiple PRs using the shard step action. Call get_codemod_troubleshooting when commands fail or produce unexpected output. Call get_codemod_creation_workflow when authoring, testing, or publishing codemods. Call get_codemod_maintainer_monorepo when setting up or maintaining a codemod monorepo.".to_string()),
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        tracing::info!("MCP server initialized");
        self.log_usage("server:initialize");
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
                    "jssg-runtime-capabilities://instructions",
                    "jssg-runtime-capabilities-instructions",
                    Some("JSSG runtime and capability guidance for LLRT/Node APIs, codemod.yaml capabilities, and multi-file JSSG work"),
                ),
                self._create_resource_text(
                    "codemod-cli://instructions",
                    "codemod-cli-instructions",
                    Some("Codemod CLI instructions for project setup and workflow configuration"),
                ),
                self._create_resource_text(
                    "sharding://instructions",
                    "sharding-instructions",
                    Some("Sharding instructions for splitting large migrations into multiple PRs"),
                ),
                self._create_resource_text(
                    "codemod-troubleshooting://instructions",
                    "codemod-troubleshooting-instructions",
                    Some("Troubleshooting guidance for common Codemod CLI failures"),
                ),
                self._create_resource_text(
                    "codemod-creation-workflow://instructions",
                    "codemod-creation-workflow-instructions",
                    Some("Codemod creation workflow guide for authoring, testing, and publishing"),
                ),
                self._create_resource_text(
                    "codemod-maintainer-monorepo://instructions",
                    "codemod-maintainer-monorepo-instructions",
                    Some("Maintainer monorepo guide for codemod repositories"),
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
                let instructions_content = build_public_docs_bundle(
                    "Canonical JSSG Documentation",
                    &[
                        JSSG_QUICKSTART_DOC_URL,
                        JSSG_REFERENCE_DOC_URL,
                        JSSG_ADVANCED_DOC_URL,
                        JSSG_TESTING_DOC_URL,
                        JSSG_UTILS_DOC_URL,
                        JSSG_SECURITY_DOC_URL,
                        JSSG_SEMANTIC_ANALYSIS_DOC_URL,
                    ],
                    JSSG_INSTRUCTIONS,
                )
                .await;
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(instructions_content, uri)],
                })
            }
            "jssg-utils://instructions" => {
                let instructions_content = build_public_docs_bundle(
                    "Canonical JSSG Import Utilities Documentation",
                    &[JSSG_UTILS_DOC_URL],
                    JSSG_UTILS_INSTRUCTIONS,
                )
                .await;
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(instructions_content, uri)],
                })
            }
            "jssg-runtime-capabilities://instructions" => {
                let instructions_content = JSSG_RUNTIME_CAPABILITIES_INSTRUCTIONS;
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(instructions_content, uri)],
                })
            }
            "codemod-cli://instructions" => {
                let instructions_content = build_public_docs_bundle(
                    "Canonical Codemod CLI and Workflow Documentation",
                    &[
                        CLI_DOC_URL,
                        PACKAGE_STRUCTURE_DOC_URL,
                        WORKFLOW_REFERENCE_DOC_URL,
                        SHARDING_DOC_URL,
                    ],
                    CODEMOD_CLI_INSTRUCTIONS,
                )
                .await;
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(instructions_content, uri)],
                })
            }
            "sharding://instructions" => {
                let instructions_content = build_public_docs_bundle(
                    "Canonical Sharding Documentation",
                    &[SHARDING_DOC_URL],
                    SHARDING_INSTRUCTIONS,
                )
                .await;
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(instructions_content, uri)],
                })
            }
            "codemod-troubleshooting://instructions" => {
                let instructions_content = include_str!("data/prompts/codemod-troubleshooting.md");
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(instructions_content, uri)],
                })
            }
            "codemod-creation-workflow://instructions" => {
                let instructions_content = build_public_docs_bundle_with_supplement(
                    "Canonical Codemod Creation Documentation",
                    &[
                        OSS_QUICKSTART_DOC_URL,
                        CLI_DOC_URL,
                        PACKAGE_STRUCTURE_DOC_URL,
                        WORKFLOW_REFERENCE_DOC_URL,
                        JSSG_QUICKSTART_DOC_URL,
                        JSSG_TESTING_DOC_URL,
                    ],
                    "Supplemental Local Guidance",
                    CODEMOD_CREATION_WORKFLOW_INSTRUCTIONS,
                    CODEMOD_CREATION_WORKFLOW_INSTRUCTIONS,
                )
                .await;
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(instructions_content, uri)],
                })
            }
            "codemod-maintainer-monorepo://instructions" => {
                let instructions_content =
                    include_str!("data/prompts/codemod-maintainer-monorepo.md");
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
    use std::fs;
    use std::sync::{LazyLock, Mutex};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    static CURRENT_DIR_GUARD: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    struct CurrentDirGuard {
        original: PathBuf,
    }

    impl CurrentDirGuard {
        fn set(path: &std::path::Path) -> Self {
            let original = std::env::current_dir().expect("expected current dir");
            std::env::set_current_dir(path).expect("expected to switch current dir");
            Self { original }
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.original).expect("expected to restore current dir");
        }
    }

    fn wait_for_usage_log(path: &std::path::Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(content) = fs::read_to_string(path) {
                if !content.trim().is_empty() {
                    return content;
                }
            }

            assert!(
                Instant::now() < deadline,
                "timed out waiting for usage log at {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[tokio::test]
    async fn test_mcp_server_creation() {
        let server = CodemodMcpServer::default();
        let info = server.get_info();

        assert_eq!(info.protocol_version, ProtocolVersion::V_2024_11_05);
        assert!(info.capabilities.tools.is_some());
        assert!(info.instructions.is_some());
        let instructions = info.instructions.as_deref().unwrap_or_default();
        assert!(instructions.contains("scaffold_codemod_package"));
        assert!(instructions.contains("get_jssg_runtime_capabilities"));
        assert!(instructions.contains("jssg-runtime-capabilities-instructions"));
        assert!(instructions.contains("validate_codemod_package"));
    }

    #[tokio::test]
    async fn test_jssg_runtime_capabilities_tool_returns_prompt() {
        let server = CodemodMcpServer::default();
        let result = server
            .get_jssg_runtime_capabilities(rmcp::handler::server::wrapper::Parameters(
                GetInstructionsRequest {},
            ))
            .await
            .expect("expected tool result");

        let serialized = serde_json::to_string(&result).expect("expected serialized tool result");
        assert!(serialized.contains("JSSG Runtime and Capabilities"));
        assert!(serialized.contains("jssgTransform"));
    }

    #[tokio::test]
    async fn test_log_usage_supports_relative_paths() {
        let _guard = CURRENT_DIR_GUARD.lock().unwrap();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("expected monotonic system time")
            .as_nanos();
        let temp_dir =
            std::env::temp_dir().join(format!("codemod-mcp-log-{}-{unique}", std::process::id()));
        fs::create_dir_all(&temp_dir).expect("expected temp dir");
        let _cwd_guard = CurrentDirGuard::set(&temp_dir);

        let relative_log_path = PathBuf::from("usage.log");
        let server = CodemodMcpServer::new(Some(relative_log_path.clone()));
        server.log_usage("tool:get_jssg_instructions");

        let content = wait_for_usage_log(&temp_dir.join(relative_log_path));
        assert!(content.contains("tool:get_jssg_instructions"));

        drop(_cwd_guard);
        fs::remove_dir_all(&temp_dir).expect("expected temp dir cleanup");
    }

    #[test]
    fn strip_docs_index_keeps_documents_that_already_start_with_h1() {
        let doc = "# Title\n\nBody";

        assert_eq!(strip_docs_index(doc), "# Title\n\nBody");
    }

    #[test]
    fn strip_docs_index_removes_docs_index_preamble() {
        let doc = "> ## Documentation Index\n> Some intro\n\n# Title\n\nBody";

        assert_eq!(strip_docs_index(doc), "# Title\n\nBody");
    }

    #[test]
    fn strip_docs_index_handles_frontmatter_before_h1() {
        let doc = "---\ntitle: Example\n---\n\n# Title\n\nBody";

        assert_eq!(strip_docs_index(doc), "# Title\n\nBody");
    }

    #[test]
    fn build_public_docs_bundle_from_sections_returns_fallback_when_empty() {
        let sections: Vec<String> = Vec::new();

        let result = build_public_docs_bundle_from_sections("Title", &sections, "fallback");

        assert_eq!(result, "fallback");
    }

    #[test]
    fn build_public_docs_bundle_with_supplement_from_sections_returns_fallback_when_empty() {
        let sections: Vec<String> = Vec::new();

        let result = build_public_docs_bundle_with_supplement_from_sections(
            "Title",
            &sections,
            "Supplement",
            "extra",
            "fallback",
        );

        assert_eq!(result, "fallback");
    }
}

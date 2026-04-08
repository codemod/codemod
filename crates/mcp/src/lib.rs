use rmcp::{
    handler::server::router::tool::ToolRouter, model::*, schemars, service::RequestContext, tool,
    tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler,
};
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod handlers;
use handlers::{
    AstDumpHandler, JssgTestHandler, NodeTypesHandler, PackageScaffoldHandler,
    PackageValidationHandler,
};

const PUBLIC_DOCS_TIMEOUT_SECS: u64 = 10;
const JSSG_INSTRUCTIONS: &str = include_str!("data/prompts/jssg-instructions.md");
const JSSG_UTILS_INSTRUCTIONS: &str = include_str!("data/prompts/jssg-utils-instructions.md");
const JSSG_RUNTIME_CAPABILITIES_INSTRUCTIONS: &str =
    include_str!("data/prompts/jssg-runtime-capabilities.md");
const CODEMOD_CLI_INSTRUCTIONS: &str = include_str!("data/prompts/codemod-cli-instructions.md");
const SHARDING_INSTRUCTIONS: &str = include_str!("data/prompts/sharding-instructions.md");
const CODEMOD_CREATION_WORKFLOW_SUPPLEMENT: &str =
    include_str!("data/prompts/codemod-creation-workflow.md");
const CODEMOD_TROUBLESHOOTING_SUPPLEMENT: &str =
    include_str!("data/prompts/codemod-troubleshooting.md");
const CODEMOD_MAINTAINER_MONOREPO_GUIDE: &str =
    include_str!("data/prompts/codemod-maintainer-monorepo.md");
const JSSG_GOTCHAS: &str = include_str!("data/prompts/jssg-gotchas.md");
const AST_GREP_GOTCHAS: &str = include_str!("data/prompts/ast-grep-gotchas.md");

const CLI_DOC_URL: &str = "https://docs.codemod.com/cli.md";
const OSS_QUICKSTART_DOC_URL: &str = "https://docs.codemod.com/oss-quickstart.md";
const PACKAGE_STRUCTURE_DOC_URL: &str = "https://docs.codemod.com/package-structure.md";
const WORKFLOW_REFERENCE_DOC_URL: &str = "https://docs.codemod.com/workflows/reference.md";
const SHARDING_DOC_URL: &str = "https://docs.codemod.com/workflows/sharding.md";
const JSSG_QUICKSTART_DOC_URL: &str = "https://docs.codemod.com/jssg/quickstart.md";
const JSSG_REFERENCE_DOC_URL: &str = "https://docs.codemod.com/jssg/reference.md";
const JSSG_ADVANCED_DOC_URL: &str = "https://docs.codemod.com/jssg/advanced.md";
const JSSG_TESTING_DOC_URL: &str = "https://docs.codemod.com/jssg/testing.md";
const JSSG_METRICS_DOC_URL: &str = "https://docs.codemod.com/jssg/metrics.md";
const JSSG_UTILS_DOC_URL: &str = "https://docs.codemod.com/jssg/utils.md";
const JSSG_SEMANTIC_ANALYSIS_DOC_URL: &str = "https://docs.codemod.com/jssg/semantic-analysis.md";

static PUBLIC_DOCS_CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();

fn public_docs_client() -> Option<&'static reqwest::Client> {
    PUBLIC_DOCS_CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(PUBLIC_DOCS_TIMEOUT_SECS))
                .user_agent(format!("codemod-mcp/{}", env!("CARGO_PKG_VERSION")))
                .build()
                .ok()
        })
        .as_ref()
}

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
    let client = public_docs_client()?;
    let response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    let text = response.text().await.ok()?;
    Some(strip_docs_index(&text).trim().to_string())
}

async fn fetch_public_doc_sections(urls: &[&str]) -> Vec<String> {
    let mut sections = Vec::new();
    for url in urls {
        if let Some(content) = fetch_public_doc_markdown(url).await {
            sections.push(format!("<!-- Source: {url} -->\n\n{content}"));
        }
    }
    sections
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
        "# {title}\n\nThese instructions are sourced from the public Codemod docs deployment (`docs.codemod.com`). Prefer this content over local fallback notes when they differ.\n\n{}\n",
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
        "# {title}\n\n## {supplement_title}\n\n{supplement}\n\n---\n\nThese instructions are sourced from the public Codemod docs deployment (`docs.codemod.com`). Prefer this content over local fallback notes when they differ.\n\n{}\n",
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
        description = "Get the highest-priority JSSG gotchas before implementing a codemod transform."
    )]
    async fn get_jssg_gotchas(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_jssg_gotchas");
        Ok(CallToolResult::success(vec![Content::text(JSSG_GOTCHAS)]))
    }

    #[tool(
        description = "Get the highest-priority ast-grep gotchas before implementing a codemod transform."
    )]
    async fn get_ast_grep_gotchas(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_ast_grep_gotchas");
        Ok(CallToolResult::success(vec![Content::text(
            AST_GREP_GOTCHAS,
        )]))
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
        description = "Validate whether a codemod package is real and complete. Use this before stopping work on a codemod package."
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

    #[tool(description = "Get JSSG guidance for creating and testing transformation scripts")]
    async fn get_jssg_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_jssg_instructions");
        let instructions_content = build_public_docs_bundle_with_supplement(
            "Canonical JSSG Documentation",
            &[
                JSSG_QUICKSTART_DOC_URL,
                JSSG_REFERENCE_DOC_URL,
                JSSG_ADVANCED_DOC_URL,
                JSSG_TESTING_DOC_URL,
                JSSG_METRICS_DOC_URL,
                JSSG_SEMANTIC_ANALYSIS_DOC_URL,
            ],
            "Agent-Specific Caveats",
            JSSG_INSTRUCTIONS,
            JSSG_INSTRUCTIONS,
        )
        .await;
        Ok(CallToolResult::success(vec![Content::text(
            instructions_content,
        )]))
    }

    #[tool(
        description = "Get JSSG import utility guidance for getImport, addImport, and removeImport"
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

    #[tool(description = "Get Codemod CLI, package, and workflow guidance")]
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
        Ok(CallToolResult::success(vec![Content::text(
            CODEMOD_TROUBLESHOOTING_SUPPLEMENT,
        )]))
    }

    #[tool(
        description = "Get codemod authoring workflow guidance for planning, scaffolding, testing, and validation"
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
                JSSG_TESTING_DOC_URL,
            ],
            "Supplemental Agent Workflow Policy",
            CODEMOD_CREATION_WORKFLOW_SUPPLEMENT,
            CODEMOD_CREATION_WORKFLOW_SUPPLEMENT,
        )
        .await;
        Ok(CallToolResult::success(vec![Content::text(
            instructions_content,
        )]))
    }

    #[tool(
        description = "Get maintainer guidance for codemod monorepos and version-hop workspaces"
    )]
    async fn get_codemod_maintainer_monorepo(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage("tool:get_codemod_maintainer_monorepo");
        Ok(CallToolResult::success(vec![Content::text(
            CODEMOD_MAINTAINER_MONOREPO_GUIDE,
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
            instructions: Some("This server provides AST dumping, tree-sitter node types, JSSG test execution, Codemod package scaffolding and validation, docs-backed JSSG/CLI/workflow guidance, and a small set of hot-path codemod gotchas. Available tools: dump_ast, get_node_types, run_jssg_tests, get_jssg_gotchas, get_ast_grep_gotchas, scaffold_codemod_package, validate_codemod_package, get_jssg_instructions, get_jssg_utils_instructions, get_jssg_runtime_capabilities, get_codemod_cli_instructions, get_codemod_creation_workflow, get_codemod_troubleshooting, get_codemod_maintainer_monorepo. Available resources: jssg-instructions, jssg-utils-instructions, jssg-runtime-capabilities-instructions, codemod-cli-instructions, sharding-instructions, codemod-troubleshooting-instructions, codemod-creation-workflow-instructions, codemod-maintainer-monorepo-instructions. For codemod authoring, call get_codemod_creation_workflow first, then get_jssg_gotchas and get_ast_grep_gotchas before writing source-transform code. If registry search finds no exact existing package, call scaffold_codemod_package immediately. Call validate_codemod_package before you stop work on a codemod package. Use dump_ast when pattern shape is unclear. If symbol origin matters, use semantic analysis and binding-aware checks.".to_string()),
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
                    Some("Docs-backed JSSG guidance with a small local supplement"),
                ),
                self._create_resource_text(
                    "jssg-utils://instructions",
                    "jssg-utils-instructions",
                    Some("Docs-backed JSSG import utility guidance"),
                ),
                self._create_resource_text(
                    "jssg-runtime-capabilities://instructions",
                    "jssg-runtime-capabilities-instructions",
                    Some("JSSG runtime and capability guidance for LLRT/Node APIs and multi-file work"),
                ),
                self._create_resource_text(
                    "codemod-cli://instructions",
                    "codemod-cli-instructions",
                    Some("Docs-backed CLI, package, and workflow guidance"),
                ),
                self._create_resource_text(
                    "sharding://instructions",
                    "sharding-instructions",
                    Some("Docs-backed sharding guidance"),
                ),
                self._create_resource_text(
                    "codemod-troubleshooting://instructions",
                    "codemod-troubleshooting-instructions",
                    Some("Local troubleshooting supplement for Codemod CLI and MCP issues"),
                ),
                self._create_resource_text(
                    "codemod-creation-workflow://instructions",
                    "codemod-creation-workflow-instructions",
                    Some("Docs-backed codemod creation guidance with a small local supplement"),
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
        let content = match uri.as_str() {
            "jssg://instructions" => {
                build_public_docs_bundle_with_supplement(
                    "Canonical JSSG Documentation",
                    &[
                        JSSG_QUICKSTART_DOC_URL,
                        JSSG_REFERENCE_DOC_URL,
                        JSSG_ADVANCED_DOC_URL,
                        JSSG_TESTING_DOC_URL,
                        JSSG_METRICS_DOC_URL,
                        JSSG_SEMANTIC_ANALYSIS_DOC_URL,
                    ],
                    "Agent-Specific Caveats",
                    JSSG_INSTRUCTIONS,
                    JSSG_INSTRUCTIONS,
                )
                .await
            }
            "jssg-utils://instructions" => {
                build_public_docs_bundle(
                    "Canonical JSSG Import Utilities Documentation",
                    &[JSSG_UTILS_DOC_URL],
                    JSSG_UTILS_INSTRUCTIONS,
                )
                .await
            }
            "jssg-runtime-capabilities://instructions" => {
                JSSG_RUNTIME_CAPABILITIES_INSTRUCTIONS.to_string()
            }
            "codemod-cli://instructions" => {
                build_public_docs_bundle(
                    "Canonical Codemod CLI and Workflow Documentation",
                    &[
                        CLI_DOC_URL,
                        PACKAGE_STRUCTURE_DOC_URL,
                        WORKFLOW_REFERENCE_DOC_URL,
                    ],
                    CODEMOD_CLI_INSTRUCTIONS,
                )
                .await
            }
            "sharding://instructions" => {
                build_public_docs_bundle(
                    "Canonical Sharding Documentation",
                    &[SHARDING_DOC_URL],
                    SHARDING_INSTRUCTIONS,
                )
                .await
            }
            "codemod-troubleshooting://instructions" => {
                CODEMOD_TROUBLESHOOTING_SUPPLEMENT.to_string()
            }
            "codemod-creation-workflow://instructions" => {
                build_public_docs_bundle_with_supplement(
                    "Canonical Codemod Creation Documentation",
                    &[
                        OSS_QUICKSTART_DOC_URL,
                        CLI_DOC_URL,
                        PACKAGE_STRUCTURE_DOC_URL,
                        WORKFLOW_REFERENCE_DOC_URL,
                        JSSG_TESTING_DOC_URL,
                    ],
                    "Supplemental Agent Workflow Policy",
                    CODEMOD_CREATION_WORKFLOW_SUPPLEMENT,
                    CODEMOD_CREATION_WORKFLOW_SUPPLEMENT,
                )
                .await
            }
            "codemod-maintainer-monorepo://instructions" => {
                CODEMOD_MAINTAINER_MONOREPO_GUIDE.to_string()
            }
            _ => {
                return Err(McpError::resource_not_found(
                    "resource_not_found",
                    Some(json!({ "uri": uri })),
                ))
            }
        };

        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text(content, uri)],
        })
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
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    static CURRENT_DIR_GUARD: OnceLock<Mutex<()>> = OnceLock::new();

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

    #[test]
    fn public_docs_bundle_falls_back_when_sections_are_empty() {
        let content = build_public_docs_bundle_from_sections("Title", &[], "fallback text");
        assert_eq!(content, "fallback text");
    }

    #[test]
    fn public_docs_bundle_with_supplement_includes_supplement() {
        let content = build_public_docs_bundle_with_supplement_from_sections(
            "Title",
            &["section".to_string()],
            "Supplement",
            "supplement text",
            "fallback",
        );
        assert!(content.contains("supplement text"));
        assert!(content.contains("section"));
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
        assert!(instructions.contains("validate_codemod_package"));
        assert!(instructions.contains("get_jssg_gotchas"));
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
        let _guard = CURRENT_DIR_GUARD
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();
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
}

use rmcp::{
    handler::server::router::tool::ToolRouter, model::*, schemars, service::RequestContext, tool,
    tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler,
};
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{de, Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::future::Future;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::OnceCell;

mod handlers;
use handlers::{AstDumpHandler, JssgTestHandler, NodeTypesHandler, PackageValidationHandler};

const PUBLIC_DOCS_TIMEOUT_SECS: u64 = 10;
const PUBLIC_DOCS_INITIAL_WAIT_MILLIS: u64 = 1000;

const LOCAL_DOCS_README: &str = include_str!(concat!(env!("OUT_DIR"), "/docs/README.md"));
const LOCAL_CLI_DOC: &str = include_str!(concat!(env!("OUT_DIR"), "/docs/cli.mdx"));
const LOCAL_MODEL_CONTEXT_PROTOCOL_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/model-context-protocol.mdx"));
const LOCAL_OSS_DOC: &str = include_str!(concat!(env!("OUT_DIR"), "/docs/oss.mdx"));
const LOCAL_OSS_QUICKSTART_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/oss-quickstart.mdx"));
const LOCAL_PACKAGE_STRUCTURE_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/package-structure.mdx"));
const LOCAL_WORKFLOW_REFERENCE_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/workflows/reference.mdx"));
const LOCAL_WORKFLOW_INTRODUCTION_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/workflows/introduction.mdx"));
const LOCAL_SHARDING_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/workflows/sharding.mdx"));
const LOCAL_JSSG_INTRO_DOC: &str = include_str!(concat!(env!("OUT_DIR"), "/docs/jssg/intro.mdx"));
const LOCAL_JSSG_REFERENCE_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/jssg/reference.mdx"));
const LOCAL_JSSG_SECURITY_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/jssg/security.mdx"));
const LOCAL_JSSG_ADVANCED_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/jssg/advanced.mdx"));
const LOCAL_JSSG_TESTING_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/jssg/testing.mdx"));
const LOCAL_JSSG_METRICS_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/jssg/metrics.mdx"));
const LOCAL_JSSG_UTILS_DOC: &str = include_str!(concat!(env!("OUT_DIR"), "/docs/jssg/utils.mdx"));
const LOCAL_JSSG_SEMANTIC_ANALYSIS_DOC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/docs/jssg/semantic-analysis.mdx"));

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
static JSSG_DOCS_BUNDLE: OnceCell<String> = OnceCell::const_new();
static JSSG_UTILS_DOCS_BUNDLE: OnceCell<String> = OnceCell::const_new();
static CODEMOD_CLI_DOCS_BUNDLE: OnceCell<String> = OnceCell::const_new();
static SHARDING_DOCS_BUNDLE: OnceCell<String> = OnceCell::const_new();
static CODEMOD_CREATION_DOCS_BUNDLE: OnceCell<String> = OnceCell::const_new();
static LOCAL_JSSG_DOCS_BUNDLE: OnceLock<String> = OnceLock::new();
static LOCAL_JSSG_GOTCHAS_DOCS_BUNDLE: OnceLock<String> = OnceLock::new();
static LOCAL_AST_GREP_GOTCHAS_DOCS_BUNDLE: OnceLock<String> = OnceLock::new();
static LOCAL_JSSG_UTILS_DOCS_BUNDLE: OnceLock<String> = OnceLock::new();
static LOCAL_JSSG_RUNTIME_CAPABILITIES_DOCS_BUNDLE: OnceLock<String> = OnceLock::new();
static LOCAL_CODEMOD_CLI_DOCS_BUNDLE: OnceLock<String> = OnceLock::new();
static LOCAL_SHARDING_DOCS_BUNDLE: OnceLock<String> = OnceLock::new();
static LOCAL_CODEMOD_TROUBLESHOOTING_DOCS_BUNDLE: OnceLock<String> = OnceLock::new();
static LOCAL_CODEMOD_CREATION_DOCS_BUNDLE: OnceLock<String> = OnceLock::new();
static LOCAL_CODEMOD_MAINTAINER_MONOREPO_DOCS_BUNDLE: OnceLock<String> = OnceLock::new();
static JSSG_DOCS_FETCH_STARTED: AtomicBool = AtomicBool::new(false);
static JSSG_UTILS_DOCS_FETCH_STARTED: AtomicBool = AtomicBool::new(false);
static CODEMOD_CLI_DOCS_FETCH_STARTED: AtomicBool = AtomicBool::new(false);
static SHARDING_DOCS_FETCH_STARTED: AtomicBool = AtomicBool::new(false);
static CODEMOD_CREATION_DOCS_FETCH_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct LocalDocSource {
    path: &'static str,
    content: &'static str,
}

#[derive(Debug, Default, schemars::JsonSchema)]
struct GetInstructionsRequest {}

impl<'de> serde::Deserialize<'de> for GetInstructionsRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct EmptyRequestVisitor;

        impl<'de> Visitor<'de> for EmptyRequestVisitor {
            type Value = GetInstructionsRequest;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("null or an empty object")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(GetInstructionsRequest {})
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(GetInstructionsRequest {})
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(GetInstructionsRequest {})
            }
        }

        deserializer.deserialize_any(EmptyRequestVisitor)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CliToolInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct CliResourceInfo {
    pub uri: &'static str,
    pub name: &'static str,
    pub description: Option<&'static str>,
    #[serde(rename = "mimeType")]
    pub mime_type: &'static str,
}

fn empty_object_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false
    })
}

fn tool_infos() -> Vec<CliToolInfo> {
    vec![
        CliToolInfo {
            name: "dump_ast",
            description: "Dump AST nodes in an AI-friendly format for the given source code and language",
            input_schema: json!({
                "type": "object",
                "required": ["source_code", "language"],
                "properties": {
                    "source_code": { "type": "string", "description": "The source code to analyze" },
                    "language": { "type": "string", "description": "The programming language, such as tsx, typescript, javascript, python, or rust" }
                },
                "additionalProperties": false
            }),
        },
        CliToolInfo {
            name: "get_node_types",
            description: "Get compressed tree-sitter node types for a specific programming language in AI-friendly format",
            input_schema: json!({
                "type": "object",
                "required": ["language"],
                "properties": {
                    "language": { "type": "string", "description": "The programming language, such as tsx, typescript, javascript, python, or rust" }
                },
                "additionalProperties": false
            }),
        },
        CliToolInfo {
            name: "run_jssg_tests",
            description: "Run tests for a jssg (JavaScript ast-grep) codemod with given test cases",
            input_schema: json!({
                "type": "object",
                "required": ["language", "codemod_file", "tests"],
                "properties": {
                    "language": { "type": "string" },
                    "codemod_file": { "type": "string" },
                    "tests": { "type": "array" },
                    "timeout_seconds": { "type": "integer", "minimum": 1 },
                    "strictness": { "type": "string" }
                },
                "additionalProperties": true
            }),
        },
        CliToolInfo {
            name: "validate_codemod_package",
            description: "Validate whether a codemod package is real and complete",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "package_path": { "type": "string" },
                    "run_default_test": { "type": "boolean" },
                    "run_check_types": { "type": "boolean" },
                    "command_timeout_seconds": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
        },
        CliToolInfo {
            name: "get_jssg_instructions",
            description: "Deprecated compatibility alias for the jssg-instructions resource",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_jssg_gotchas",
            description: "Deprecated compatibility alias for the jssg-gotchas resource",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_ast_grep_gotchas",
            description: "Deprecated compatibility alias for the ast-grep-gotchas resource",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_jssg_utils_instructions",
            description: "Deprecated compatibility alias for the jssg-utils-instructions resource",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_jssg_runtime_capabilities_instructions",
            description: "Deprecated compatibility alias for the jssg-runtime-capabilities-instructions resource",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_jssg_runtime_capabilities",
            description: "Deprecated compatibility alias for get_jssg_runtime_capabilities_instructions",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_codemod_cli_instructions",
            description: "Deprecated compatibility alias for the codemod-cli-instructions resource",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_sharding_instructions",
            description: "Deprecated compatibility alias for the sharding-instructions resource",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_codemod_troubleshooting_instructions",
            description: "Deprecated compatibility alias for the codemod-troubleshooting-instructions resource",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_codemod_troubleshooting",
            description: "Deprecated compatibility alias for get_codemod_troubleshooting_instructions",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_codemod_creation_workflow_instructions",
            description: "Deprecated compatibility alias for the codemod-creation-workflow-instructions resource",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_codemod_creation_workflow",
            description: "Deprecated compatibility alias for get_codemod_creation_workflow_instructions",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_codemod_maintainer_monorepo_instructions",
            description: "Deprecated compatibility alias for the codemod-maintainer-monorepo-instructions resource",
            input_schema: empty_object_schema(),
        },
        CliToolInfo {
            name: "get_codemod_maintainer_monorepo",
            description: "Deprecated compatibility alias for get_codemod_maintainer_monorepo_instructions",
            input_schema: empty_object_schema(),
        },
    ]
}

fn resource_infos() -> &'static [CliResourceInfo] {
    &[
        CliResourceInfo {
            uri: "jssg://instructions",
            name: "jssg-instructions",
            description: Some("Docs-backed JSSG guidance"),
            mime_type: "text/markdown",
        },
        CliResourceInfo {
            uri: "jssg-gotchas://instructions",
            name: "jssg-gotchas",
            description: Some("Highest-priority JSSG gotchas for codemod authoring"),
            mime_type: "text/markdown",
        },
        CliResourceInfo {
            uri: "ast-grep-gotchas://instructions",
            name: "ast-grep-gotchas",
            description: Some("Highest-priority ast-grep gotchas for codemod authoring"),
            mime_type: "text/markdown",
        },
        CliResourceInfo {
            uri: "jssg-utils://instructions",
            name: "jssg-utils-instructions",
            description: Some("Docs-backed JSSG utility guidance"),
            mime_type: "text/markdown",
        },
        CliResourceInfo {
            uri: "jssg-runtime-capabilities://instructions",
            name: "jssg-runtime-capabilities-instructions",
            description: Some(
                "JSSG runtime and capability guidance for LLRT/Node APIs and multi-file work",
            ),
            mime_type: "text/markdown",
        },
        CliResourceInfo {
            uri: "codemod-cli://instructions",
            name: "codemod-cli-instructions",
            description: Some("Docs-backed CLI, package, and workflow guidance"),
            mime_type: "text/markdown",
        },
        CliResourceInfo {
            uri: "sharding://instructions",
            name: "sharding-instructions",
            description: Some("Docs-backed sharding guidance"),
            mime_type: "text/markdown",
        },
        CliResourceInfo {
            uri: "codemod-troubleshooting://instructions",
            name: "codemod-troubleshooting-instructions",
            description: Some(
                "Docs-backed troubleshooting guidance for Codemod CLI and MCP issues",
            ),
            mime_type: "text/markdown",
        },
        CliResourceInfo {
            uri: "codemod-creation-workflow://instructions",
            name: "codemod-creation-workflow-instructions",
            description: Some("Docs-backed codemod creation guidance"),
            mime_type: "text/markdown",
        },
        CliResourceInfo {
            uri: "codemod-maintainer-monorepo://instructions",
            name: "codemod-maintainer-monorepo-instructions",
            description: Some("Maintainer monorepo guide for codemod repositories"),
            mime_type: "text/markdown",
        },
    ]
}

fn normalize_cli_tool_name(tool_name: &str) -> String {
    match tool_name {
        "dump-ast" => "dump_ast".to_string(),
        "node-types" => "get_node_types".to_string(),
        name => name.replace('-', "_"),
    }
}

fn call_tool_result_text(result: CallToolResult) -> String {
    result
        .content
        .into_iter()
        .map(|content| {
            content
                .as_text()
                .map(|text| text.text.clone())
                .unwrap_or_else(|| format!("{content:?}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Debug)]
pub struct AnonymousFeedbackClient {
    endpoint: String,
    source: &'static str,
    cli_version: String,
    consented_at: Option<String>,
    os: &'static str,
    arch: &'static str,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct AnonymousFeedbackPayload<'a> {
    source: &'a str,
    event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(rename = "consentedAt", skip_serializing_if = "Option::is_none")]
    consented_at: Option<&'a str>,
    #[serde(rename = "cliVersion")]
    cli_version: &'a str,
    os: &'a str,
    arch: &'a str,
    metadata: HashMap<String, String>,
}

impl AnonymousFeedbackClient {
    pub fn new(
        endpoint: String,
        source: &'static str,
        cli_version: String,
        consented_at: Option<String>,
    ) -> Option<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .ok()?;

        Some(Self {
            endpoint,
            source,
            cli_version,
            consented_at,
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            client,
        })
    }

    pub async fn submit(&self, event: &str, metadata: HashMap<String, String>) {
        let _ = self.submit_feedback(event, None, None, metadata).await;
    }

    pub async fn submit_feedback(
        &self,
        event: &str,
        category: Option<String>,
        message: Option<String>,
        metadata: HashMap<String, String>,
    ) -> Result<(), String> {
        let payload = AnonymousFeedbackPayload {
            source: self.source,
            event: sanitize_feedback_event(event),
            category,
            message,
            consented_at: self.consented_at.as_deref(),
            cli_version: &self.cli_version,
            os: self.os,
            arch: self.arch,
            metadata,
        };

        let response = self
            .client
            .post(&self.endpoint)
            .header(
                reqwest::header::USER_AGENT,
                format!("codemod-cli/{}", self.cli_version),
            )
            .json(&payload)
            .send()
            .await
            .map_err(|error| format!("request failed: {error}"))?;

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }

        let body = response
            .text()
            .await
            .unwrap_or_else(|error| format!("failed to read response body: {error}"));

        Err(format!("server returned {status}: {body}"))
    }
}

fn sanitize_feedback_event(event: &str) -> String {
    let sanitized = event
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, ':' | '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();

    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.chars().take(128).collect()
    }
}

fn public_docs_client() -> Option<&'static reqwest::Client> {
    PUBLIC_DOCS_CLIENT
        .get_or_init(|| {
            if std::env::var_os("CODEMOD_MCP_PUBLIC_DOCS_OFFLINE").is_some() {
                return None;
            }

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
    let mut content_start = line_start;

    if line == "---" {
        let mut cursor = line_end;
        while let Some((_, end, candidate)) = next_nonempty_line(doc, cursor) {
            cursor = end;
            if candidate == "---" {
                if let Some((start, end, candidate)) = next_nonempty_line(doc, cursor) {
                    line_start = start;
                    line_end = end;
                    line = candidate;
                    content_start = start;
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

    &doc[content_start..]
}

fn local_doc_section(source: LocalDocSource) -> Option<String> {
    let content = strip_docs_index(source.content).trim();
    if content.is_empty() {
        return None;
    }

    Some(format!(
        "<!-- Local source: {} -->\n\n{content}",
        source.path
    ))
}

fn local_doc_sections(sources: &[LocalDocSource]) -> Vec<String> {
    sources
        .iter()
        .copied()
        .filter_map(local_doc_section)
        .collect()
}

fn build_docs_bundle_from_sections(
    title: &str,
    source_note: &str,
    sections: &[String],
    fallback: &str,
) -> String {
    if sections.is_empty() {
        return fallback.to_string();
    }

    format!(
        "# {title}\n\n{source_note}\n\n{}\n",
        sections.join("\n\n---\n\n")
    )
}

fn build_local_docs_bundle(title: &str, sources: &[LocalDocSource]) -> String {
    let sections = local_doc_sections(sources);
    build_docs_bundle_from_sections(
        title,
        "These instructions are bundled from this release's local `docs/` directory.",
        &sections,
        "",
    )
}

fn local_jssg_docs_bundle() -> &'static str {
    LOCAL_JSSG_DOCS_BUNDLE
        .get_or_init(|| {
            build_local_docs_bundle(
                "Canonical JSSG Documentation",
                &[
                    LocalDocSource {
                        path: "docs/jssg/intro.mdx",
                        content: LOCAL_JSSG_INTRO_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/reference.mdx",
                        content: LOCAL_JSSG_REFERENCE_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/advanced.mdx",
                        content: LOCAL_JSSG_ADVANCED_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/testing.mdx",
                        content: LOCAL_JSSG_TESTING_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/metrics.mdx",
                        content: LOCAL_JSSG_METRICS_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/semantic-analysis.mdx",
                        content: LOCAL_JSSG_SEMANTIC_ANALYSIS_DOC,
                    },
                ],
            )
        })
        .as_str()
}

fn local_jssg_gotchas_docs_bundle() -> &'static str {
    LOCAL_JSSG_GOTCHAS_DOCS_BUNDLE
        .get_or_init(|| {
            build_local_docs_bundle(
                "Canonical JSSG Gotchas Documentation",
                &[
                    LocalDocSource {
                        path: "docs/jssg/intro.mdx",
                        content: LOCAL_JSSG_INTRO_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/reference.mdx",
                        content: LOCAL_JSSG_REFERENCE_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/advanced.mdx",
                        content: LOCAL_JSSG_ADVANCED_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/testing.mdx",
                        content: LOCAL_JSSG_TESTING_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/security.mdx",
                        content: LOCAL_JSSG_SECURITY_DOC,
                    },
                ],
            )
        })
        .as_str()
}

fn local_ast_grep_gotchas_docs_bundle() -> &'static str {
    LOCAL_AST_GREP_GOTCHAS_DOCS_BUNDLE
        .get_or_init(|| {
            build_local_docs_bundle(
                "Canonical ast-grep Usage Documentation",
                &[
                    LocalDocSource {
                        path: "docs/jssg/intro.mdx",
                        content: LOCAL_JSSG_INTRO_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/reference.mdx",
                        content: LOCAL_JSSG_REFERENCE_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/advanced.mdx",
                        content: LOCAL_JSSG_ADVANCED_DOC,
                    },
                ],
            )
        })
        .as_str()
}

fn local_jssg_utils_docs_bundle() -> &'static str {
    LOCAL_JSSG_UTILS_DOCS_BUNDLE
        .get_or_init(|| {
            build_local_docs_bundle(
                "Canonical JSSG Utilities Documentation",
                &[LocalDocSource {
                    path: "docs/jssg/utils.mdx",
                    content: LOCAL_JSSG_UTILS_DOC,
                }],
            )
        })
        .as_str()
}

fn local_jssg_runtime_capabilities_docs_bundle() -> &'static str {
    LOCAL_JSSG_RUNTIME_CAPABILITIES_DOCS_BUNDLE
        .get_or_init(|| {
            build_local_docs_bundle(
                "Canonical JSSG Runtime Capabilities Documentation",
                &[
                    LocalDocSource {
                        path: "docs/jssg/reference.mdx",
                        content: LOCAL_JSSG_REFERENCE_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/security.mdx",
                        content: LOCAL_JSSG_SECURITY_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/advanced.mdx",
                        content: LOCAL_JSSG_ADVANCED_DOC,
                    },
                ],
            )
        })
        .as_str()
}

fn local_codemod_cli_docs_bundle() -> &'static str {
    LOCAL_CODEMOD_CLI_DOCS_BUNDLE
        .get_or_init(|| {
            build_local_docs_bundle(
                "Canonical Codemod CLI and Workflow Documentation",
                &[
                    LocalDocSource {
                        path: "docs/cli.mdx",
                        content: LOCAL_CLI_DOC,
                    },
                    LocalDocSource {
                        path: "docs/package-structure.mdx",
                        content: LOCAL_PACKAGE_STRUCTURE_DOC,
                    },
                    LocalDocSource {
                        path: "docs/workflows/reference.mdx",
                        content: LOCAL_WORKFLOW_REFERENCE_DOC,
                    },
                ],
            )
        })
        .as_str()
}

fn local_sharding_docs_bundle() -> &'static str {
    LOCAL_SHARDING_DOCS_BUNDLE
        .get_or_init(|| {
            build_local_docs_bundle(
                "Canonical Sharding Documentation",
                &[LocalDocSource {
                    path: "docs/workflows/sharding.mdx",
                    content: LOCAL_SHARDING_DOC,
                }],
            )
        })
        .as_str()
}

fn local_codemod_troubleshooting_docs_bundle() -> &'static str {
    LOCAL_CODEMOD_TROUBLESHOOTING_DOCS_BUNDLE
        .get_or_init(|| {
            build_local_docs_bundle(
                "Canonical Codemod Troubleshooting Documentation",
                &[
                    LocalDocSource {
                        path: "docs/model-context-protocol.mdx",
                        content: LOCAL_MODEL_CONTEXT_PROTOCOL_DOC,
                    },
                    LocalDocSource {
                        path: "docs/cli.mdx",
                        content: LOCAL_CLI_DOC,
                    },
                    LocalDocSource {
                        path: "docs/oss-quickstart.mdx",
                        content: LOCAL_OSS_QUICKSTART_DOC,
                    },
                ],
            )
        })
        .as_str()
}

fn local_codemod_creation_docs_bundle() -> &'static str {
    LOCAL_CODEMOD_CREATION_DOCS_BUNDLE
        .get_or_init(|| {
            build_local_docs_bundle(
                "Canonical Codemod Creation Documentation",
                &[
                    LocalDocSource {
                        path: "docs/oss-quickstart.mdx",
                        content: LOCAL_OSS_QUICKSTART_DOC,
                    },
                    LocalDocSource {
                        path: "docs/cli.mdx",
                        content: LOCAL_CLI_DOC,
                    },
                    LocalDocSource {
                        path: "docs/package-structure.mdx",
                        content: LOCAL_PACKAGE_STRUCTURE_DOC,
                    },
                    LocalDocSource {
                        path: "docs/workflows/reference.mdx",
                        content: LOCAL_WORKFLOW_REFERENCE_DOC,
                    },
                    LocalDocSource {
                        path: "docs/jssg/testing.mdx",
                        content: LOCAL_JSSG_TESTING_DOC,
                    },
                ],
            )
        })
        .as_str()
}

fn local_codemod_maintainer_monorepo_docs_bundle() -> &'static str {
    LOCAL_CODEMOD_MAINTAINER_MONOREPO_DOCS_BUNDLE
        .get_or_init(|| {
            build_local_docs_bundle(
                "Canonical Codemod Maintainer Documentation",
                &[
                    LocalDocSource {
                        path: "docs/README.md",
                        content: LOCAL_DOCS_README,
                    },
                    LocalDocSource {
                        path: "docs/oss.mdx",
                        content: LOCAL_OSS_DOC,
                    },
                    LocalDocSource {
                        path: "docs/workflows/introduction.mdx",
                        content: LOCAL_WORKFLOW_INTRODUCTION_DOC,
                    },
                    LocalDocSource {
                        path: "docs/workflows/reference.mdx",
                        content: LOCAL_WORKFLOW_REFERENCE_DOC,
                    },
                    LocalDocSource {
                        path: "docs/package-structure.mdx",
                        content: LOCAL_PACKAGE_STRUCTURE_DOC,
                    },
                ],
            )
        })
        .as_str()
}

async fn fetch_public_doc_markdown(url: &str) -> Option<String> {
    let client = public_docs_client()?;
    let response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    let text = response.text().await.ok()?;
    Some(strip_docs_index(&text).trim().to_string())
}

async fn fetch_public_doc_sections(urls: &[&str]) -> Vec<String> {
    let mut tasks = tokio::task::JoinSet::new();
    for (index, url) in urls.iter().copied().enumerate() {
        let url = url.to_string();
        tasks.spawn(async move {
            let content = fetch_public_doc_markdown(&url).await;
            (index, url, content)
        });
    }

    let mut sections = vec![None; urls.len()];
    while let Some(result) = tasks.join_next().await {
        if let Ok((index, url, Some(content))) = result {
            sections[index] = Some(format!("<!-- Source: {url} -->\n\n{content}"));
        }
    }

    sections.into_iter().flatten().collect()
}

async fn cached_public_docs_bundle<F, Fut>(
    cell: &'static OnceCell<String>,
    fetch_started: &'static AtomicBool,
    fallback: &'static str,
    initial_wait: Duration,
    build: F,
) -> String
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = String> + Send + 'static,
{
    if let Some(cached) = cell.get() {
        return cached.clone();
    }

    if initial_wait.is_zero() {
        return fallback.to_string();
    }

    if fetch_started
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let future = build();
        tokio::spawn(async move {
            let content = future.await;
            let _ = sender.send(content.clone());
            let _ = cell.set(content);
        });

        if let Ok(Ok(content)) = tokio::time::timeout(initial_wait, receiver).await {
            return content;
        }
    } else if !initial_wait.is_zero() {
        let deadline = Instant::now() + initial_wait;
        while Instant::now() < deadline {
            if let Some(cached) = cell.get() {
                return cached.clone();
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            tokio::time::sleep(remaining.min(Duration::from_millis(20))).await;
        }
    }

    fallback.to_string()
}

fn build_public_docs_bundle_from_sections(
    title: &str,
    sections: &[String],
    fallback: &str,
) -> String {
    build_docs_bundle_from_sections(
        title,
        "These instructions are sourced from the public Codemod docs deployment (`docs.codemod.com`).",
        sections,
        fallback,
    )
}

async fn build_public_docs_bundle(title: &str, urls: &[&str], fallback: &str) -> String {
    let sections = fetch_public_doc_sections(urls).await;
    build_public_docs_bundle_from_sections(title, &sections, fallback)
}

#[derive(Clone)]
pub struct CodemodMcpServer {
    ast_dump_handler: AstDumpHandler,
    node_types_handler: NodeTypesHandler,
    jssg_test_handler: JssgTestHandler,
    package_validation_handler: PackageValidationHandler,
    usage_log_path: Option<PathBuf>,
    anonymous_feedback: Option<AnonymousFeedbackClient>,
    tool_router: ToolRouter<CodemodMcpServer>,
}

impl Default for CodemodMcpServer {
    fn default() -> Self {
        Self::new(None)
    }
}

impl CodemodMcpServer {
    pub fn new(usage_log_path: Option<PathBuf>) -> Self {
        Self::new_with_feedback(usage_log_path, None)
    }

    pub fn new_with_feedback(
        usage_log_path: Option<PathBuf>,
        anonymous_feedback: Option<AnonymousFeedbackClient>,
    ) -> Self {
        Self {
            ast_dump_handler: AstDumpHandler::new(),
            node_types_handler: NodeTypesHandler::new(),
            jssg_test_handler: JssgTestHandler::new(),
            package_validation_handler: PackageValidationHandler::new(),
            usage_log_path,
            anonymous_feedback,
            tool_router: Self::tool_router(),
        }
    }

    pub fn cli_tools(&self) -> Vec<CliToolInfo> {
        tool_infos()
    }

    pub fn cli_resources(&self) -> Vec<CliResourceInfo> {
        resource_infos().to_vec()
    }

    pub async fn dump_ast_text(&self, source_code: &str, language: &str) -> anyhow::Result<String> {
        self.log_usage("cli:dump_ast");
        self.ast_dump_handler
            .dump_ast_text(source_code, language)
            .map_err(|error| anyhow::anyhow!(error))
    }

    pub async fn node_types_text(&self, language: &str) -> anyhow::Result<String> {
        self.log_usage("cli:get_node_types");
        self.node_types_handler
            .get_node_types_text(language)
            .map_err(|error| anyhow::anyhow!(error))
    }

    pub async fn read_resource_text(&self, uri: &str) -> anyhow::Result<String> {
        self.log_usage(&format!("cli:resource:{uri}"));
        self.resource_content(uri)
            .await
            .map_err(|error| anyhow::anyhow!("{error:?}"))
    }

    pub async fn read_resource_text_cached(&self, uri: &str) -> anyhow::Result<String> {
        self.log_usage(&format!("cli:resource-cached:{uri}"));
        self.resource_content_with_wait(uri, Duration::ZERO)
            .await
            .map_err(|error| anyhow::anyhow!("{error:?}"))
    }

    pub async fn read_resource_text_live(&self, uri: &str) -> anyhow::Result<String> {
        self.log_usage(&format!("cli:resource-live:{uri}"));
        self.resource_content_with_wait(uri, Duration::from_secs(PUBLIC_DOCS_TIMEOUT_SECS + 1))
            .await
            .map_err(|error| anyhow::anyhow!("{error:?}"))
    }

    pub async fn call_tool_text(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> anyhow::Result<String> {
        let normalized_name = normalize_cli_tool_name(tool_name);
        self.log_usage(&format!("cli:tool:{normalized_name}"));

        match normalized_name.as_str() {
            "dump_ast" => {
                #[derive(Deserialize)]
                struct Request {
                    source_code: String,
                    language: String,
                }
                let request: Request = serde_json::from_value(arguments)?;
                self.dump_ast_text(&request.source_code, &request.language)
                    .await
            }
            "get_node_types" => {
                #[derive(Deserialize)]
                struct Request {
                    language: String,
                }
                let request: Request = serde_json::from_value(arguments)?;
                self.node_types_text(&request.language).await
            }
            "run_jssg_tests" => {
                let request = serde_json::from_value(arguments)?;
                let result = self
                    .jssg_test_handler
                    .run_jssg_tests(rmcp::handler::server::wrapper::Parameters(request))
                    .await
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?;
                Ok(call_tool_result_text(result))
            }
            "validate_codemod_package" => {
                let request = serde_json::from_value(arguments)?;
                let result = self
                    .package_validation_handler
                    .validate_codemod_package(rmcp::handler::server::wrapper::Parameters(request))
                    .await
                    .map_err(|error| anyhow::anyhow!("{error:?}"))?;
                Ok(call_tool_result_text(result))
            }
            "get_jssg_instructions" => self.read_resource_text("jssg://instructions").await,
            "get_jssg_gotchas" => self.read_resource_text("jssg-gotchas://instructions").await,
            "get_ast_grep_gotchas" => {
                self.read_resource_text("ast-grep-gotchas://instructions")
                    .await
            }
            "get_jssg_utils_instructions" => {
                self.read_resource_text("jssg-utils://instructions").await
            }
            "get_jssg_runtime_capabilities_instructions" | "get_jssg_runtime_capabilities" => {
                self.read_resource_text("jssg-runtime-capabilities://instructions")
                    .await
            }
            "get_codemod_cli_instructions" => {
                self.read_resource_text("codemod-cli://instructions").await
            }
            "get_sharding_instructions" => self.read_resource_text("sharding://instructions").await,
            "get_codemod_troubleshooting_instructions" | "get_codemod_troubleshooting" => {
                self.read_resource_text("codemod-troubleshooting://instructions")
                    .await
            }
            "get_codemod_creation_workflow_instructions" | "get_codemod_creation_workflow" => {
                self.read_resource_text("codemod-creation-workflow://instructions")
                    .await
            }
            "get_codemod_maintainer_monorepo_instructions" | "get_codemod_maintainer_monorepo" => {
                self.read_resource_text("codemod-maintainer-monorepo://instructions")
                    .await
            }
            _ => Err(anyhow::anyhow!("Unknown MCP tool '{tool_name}'")),
        }
    }

    fn log_usage(&self, event: &str) {
        if let Some(feedback) = self.anonymous_feedback.clone() {
            let event = event.to_string();
            let _handle = tokio::spawn(async move {
                feedback.submit(&event, HashMap::new()).await;
            });
        }

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

    async fn instruction_tool_response(
        &self,
        uri: &str,
        event: &str,
    ) -> Result<CallToolResult, McpError> {
        self.log_usage(event);
        let content = self.resource_content(uri).await?;
        Ok(CallToolResult::success(vec![Content::text(content)]))
    }

    fn resources(&self) -> Vec<Resource> {
        resource_infos()
            .iter()
            .map(|resource| {
                self._create_resource_text(resource.uri, resource.name, resource.description)
            })
            .collect()
    }

    async fn resource_content(&self, uri: &str) -> Result<String, McpError> {
        self.resource_content_with_wait(uri, Duration::from_millis(PUBLIC_DOCS_INITIAL_WAIT_MILLIS))
            .await
    }

    async fn resource_content_with_wait(
        &self,
        uri: &str,
        initial_wait: Duration,
    ) -> Result<String, McpError> {
        match uri {
            "jssg://instructions" => {
                let fallback = local_jssg_docs_bundle();
                Ok(cached_public_docs_bundle(
                    &JSSG_DOCS_BUNDLE,
                    &JSSG_DOCS_FETCH_STARTED,
                    fallback,
                    initial_wait,
                    move || async move {
                        build_public_docs_bundle(
                            "Canonical JSSG Documentation",
                            &[
                                JSSG_QUICKSTART_DOC_URL,
                                JSSG_REFERENCE_DOC_URL,
                                JSSG_ADVANCED_DOC_URL,
                                JSSG_TESTING_DOC_URL,
                                JSSG_METRICS_DOC_URL,
                                JSSG_SEMANTIC_ANALYSIS_DOC_URL,
                            ],
                            fallback,
                        )
                        .await
                    },
                )
                .await)
            }
            "jssg-gotchas://instructions" => Ok(local_jssg_gotchas_docs_bundle().to_string()),
            "ast-grep-gotchas://instructions" => {
                Ok(local_ast_grep_gotchas_docs_bundle().to_string())
            }
            "jssg-utils://instructions" => {
                let fallback = local_jssg_utils_docs_bundle();
                Ok(cached_public_docs_bundle(
                    &JSSG_UTILS_DOCS_BUNDLE,
                    &JSSG_UTILS_DOCS_FETCH_STARTED,
                    fallback,
                    initial_wait,
                    move || async move {
                        build_public_docs_bundle(
                            "Canonical JSSG Import Utilities Documentation",
                            &[JSSG_UTILS_DOC_URL],
                            fallback,
                        )
                        .await
                    },
                )
                .await)
            }
            "jssg-runtime-capabilities://instructions" => {
                Ok(local_jssg_runtime_capabilities_docs_bundle().to_string())
            }
            "codemod-cli://instructions" => {
                let fallback = local_codemod_cli_docs_bundle();
                Ok(cached_public_docs_bundle(
                    &CODEMOD_CLI_DOCS_BUNDLE,
                    &CODEMOD_CLI_DOCS_FETCH_STARTED,
                    fallback,
                    initial_wait,
                    move || async move {
                        build_public_docs_bundle(
                            "Canonical Codemod CLI and Workflow Documentation",
                            &[
                                CLI_DOC_URL,
                                PACKAGE_STRUCTURE_DOC_URL,
                                WORKFLOW_REFERENCE_DOC_URL,
                            ],
                            fallback,
                        )
                        .await
                    },
                )
                .await)
            }
            "sharding://instructions" => {
                let fallback = local_sharding_docs_bundle();
                Ok(cached_public_docs_bundle(
                    &SHARDING_DOCS_BUNDLE,
                    &SHARDING_DOCS_FETCH_STARTED,
                    fallback,
                    initial_wait,
                    move || async move {
                        build_public_docs_bundle(
                            "Canonical Sharding Documentation",
                            &[SHARDING_DOC_URL],
                            fallback,
                        )
                        .await
                    },
                )
                .await)
            }
            "codemod-troubleshooting://instructions" => {
                Ok(local_codemod_troubleshooting_docs_bundle().to_string())
            }
            "codemod-creation-workflow://instructions" => {
                let fallback = local_codemod_creation_docs_bundle();
                Ok(cached_public_docs_bundle(
                    &CODEMOD_CREATION_DOCS_BUNDLE,
                    &CODEMOD_CREATION_DOCS_FETCH_STARTED,
                    fallback,
                    initial_wait,
                    move || async move {
                        build_public_docs_bundle(
                            "Canonical Codemod Creation Documentation",
                            &[
                                OSS_QUICKSTART_DOC_URL,
                                CLI_DOC_URL,
                                PACKAGE_STRUCTURE_DOC_URL,
                                WORKFLOW_REFERENCE_DOC_URL,
                                JSSG_TESTING_DOC_URL,
                            ],
                            fallback,
                        )
                        .await
                    },
                )
                .await)
            }
            "codemod-maintainer-monorepo://instructions" => {
                Ok(local_codemod_maintainer_monorepo_docs_bundle().to_string())
            }
            _ => Err(McpError::resource_not_found(
                "resource_not_found",
                Some(json!({ "uri": uri })),
            )),
        }
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

    #[tool(
        description = "Deprecated compatibility alias for jssg instructions. Prefer the jssg-instructions resource."
    )]
    async fn get_jssg_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response("jssg://instructions", "tool:get_jssg_instructions")
            .await
    }

    #[tool(
        description = "Deprecated compatibility alias for jssg gotchas. Prefer the jssg-gotchas resource."
    )]
    async fn get_jssg_gotchas(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response("jssg-gotchas://instructions", "tool:get_jssg_gotchas")
            .await
    }

    #[tool(
        description = "Deprecated compatibility alias for ast-grep gotchas. Prefer the ast-grep-gotchas resource."
    )]
    async fn get_ast_grep_gotchas(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response(
            "ast-grep-gotchas://instructions",
            "tool:get_ast_grep_gotchas",
        )
        .await
    }

    #[tool(
        description = "Deprecated compatibility alias for jssg utils instructions. Prefer the jssg-utils-instructions resource."
    )]
    async fn get_jssg_utils_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response(
            "jssg-utils://instructions",
            "tool:get_jssg_utils_instructions",
        )
        .await
    }

    #[tool(
        description = "Deprecated compatibility alias for JSSG runtime capabilities instructions. Prefer the jssg-runtime-capabilities-instructions resource."
    )]
    async fn get_jssg_runtime_capabilities_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response(
            "jssg-runtime-capabilities://instructions",
            "tool:get_jssg_runtime_capabilities_instructions",
        )
        .await
    }

    #[tool(
        description = "Deprecated compatibility alias for get_jssg_runtime_capabilities_instructions."
    )]
    async fn get_jssg_runtime_capabilities(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response(
            "jssg-runtime-capabilities://instructions",
            "tool:get_jssg_runtime_capabilities",
        )
        .await
    }

    #[tool(
        description = "Deprecated compatibility alias for codemod CLI instructions. Prefer the codemod-cli-instructions resource."
    )]
    async fn get_codemod_cli_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response(
            "codemod-cli://instructions",
            "tool:get_codemod_cli_instructions",
        )
        .await
    }

    #[tool(
        description = "Deprecated compatibility alias for sharding instructions. Prefer the sharding-instructions resource."
    )]
    async fn get_sharding_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response("sharding://instructions", "tool:get_sharding_instructions")
            .await
    }

    #[tool(
        description = "Deprecated compatibility alias for codemod troubleshooting instructions. Prefer the codemod-troubleshooting-instructions resource."
    )]
    async fn get_codemod_troubleshooting_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response(
            "codemod-troubleshooting://instructions",
            "tool:get_codemod_troubleshooting_instructions",
        )
        .await
    }

    #[tool(
        description = "Deprecated compatibility alias for get_codemod_troubleshooting_instructions."
    )]
    async fn get_codemod_troubleshooting(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response(
            "codemod-troubleshooting://instructions",
            "tool:get_codemod_troubleshooting",
        )
        .await
    }

    #[tool(
        description = "Deprecated compatibility alias for codemod creation workflow instructions. Prefer the codemod-creation-workflow-instructions resource."
    )]
    async fn get_codemod_creation_workflow_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response(
            "codemod-creation-workflow://instructions",
            "tool:get_codemod_creation_workflow_instructions",
        )
        .await
    }

    #[tool(
        description = "Deprecated compatibility alias for get_codemod_creation_workflow_instructions."
    )]
    async fn get_codemod_creation_workflow(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response(
            "codemod-creation-workflow://instructions",
            "tool:get_codemod_creation_workflow",
        )
        .await
    }

    #[tool(
        description = "Deprecated compatibility alias for codemod maintainer monorepo instructions. Prefer the codemod-maintainer-monorepo-instructions resource."
    )]
    async fn get_codemod_maintainer_monorepo_instructions(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response(
            "codemod-maintainer-monorepo://instructions",
            "tool:get_codemod_maintainer_monorepo_instructions",
        )
        .await
    }

    #[tool(
        description = "Deprecated compatibility alias for get_codemod_maintainer_monorepo_instructions."
    )]
    async fn get_codemod_maintainer_monorepo(
        &self,
        _params: rmcp::handler::server::wrapper::Parameters<GetInstructionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        self.instruction_tool_response(
            "codemod-maintainer-monorepo://instructions",
            "tool:get_codemod_maintainer_monorepo",
        )
        .await
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
            instructions: Some("This server provides AST dumping, tree-sitter node types, JSSG test execution, and Codemod package validation. Available tools: dump_ast, get_node_types, run_jssg_tests, validate_codemod_package. Available resources: jssg-instructions, jssg-gotchas, ast-grep-gotchas, jssg-utils-instructions, jssg-runtime-capabilities-instructions, codemod-cli-instructions, sharding-instructions, codemod-troubleshooting-instructions, codemod-creation-workflow-instructions, codemod-maintainer-monorepo-instructions. For codemod authoring, read codemod-creation-workflow-instructions first, then read jssg-gotchas and ast-grep-gotchas before writing source-transform code. If registry search finds no exact existing package, run direct codemod init immediately; in non-interactive flows, pass only user- or task-provided metadata flags and rely on CLI defaults/auth-derived author handling for the rest. Call validate_codemod_package before you stop work on a codemod package. Use dump_ast when pattern shape is unclear. If symbol origin matters, use semantic analysis and binding-aware checks. If you discover a Codemod platform gap, ask the user for explicit consent before submitting anonymous feedback. With consent, run `codemod ai feedback --category <category> --message <short message>` using categories like jssg, workflow, ai-docs, mcp, cli, registry, package-validation, or other. Do not include source code, secrets, auth tokens, private repository paths, user identity, or long transcripts.".to_string()),
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
            resources: self.resources(),
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        ReadResourceRequestParam { uri }: ReadResourceRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let content = self.resource_content(uri.as_str()).await?;

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
    fn local_codemod_cli_docs_bundle_uses_repo_docs() {
        let content = local_codemod_cli_docs_bundle();

        assert!(content.contains(
            "These instructions are bundled from this release's local `docs/` directory."
        ));
        assert!(content.contains("<!-- Local source: docs/cli.mdx -->"));
        assert!(content.contains("CLI Command Reference"));
    }

    #[tokio::test]
    async fn cached_public_docs_bundle_returns_fetched_content_when_fast() {
        let cell = Box::leak(Box::new(OnceCell::new()));
        let started = Box::leak(Box::new(AtomicBool::new(false)));

        let content = cached_public_docs_bundle(
            cell,
            started,
            "fallback",
            Duration::from_millis(PUBLIC_DOCS_INITIAL_WAIT_MILLIS),
            || async { "fetched".to_string() },
        )
        .await;

        assert_eq!(content, "fetched");
        assert_eq!(cell.get().map(String::as_str), Some("fetched"));
    }

    #[tokio::test]
    async fn cached_public_docs_bundle_falls_back_when_fetch_is_slow() {
        let cell = Box::leak(Box::new(OnceCell::new()));
        let started = Box::leak(Box::new(AtomicBool::new(false)));

        let content = cached_public_docs_bundle(
            cell,
            started,
            "fallback",
            Duration::from_millis(PUBLIC_DOCS_INITIAL_WAIT_MILLIS),
            || async {
                tokio::time::sleep(Duration::from_millis(PUBLIC_DOCS_INITIAL_WAIT_MILLIS * 2))
                    .await;
                "fetched".to_string()
            },
        )
        .await;

        assert_eq!(content, "fallback");

        let deadline = Instant::now() + Duration::from_secs(2);
        while cell.get().is_none() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for cached content"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(cell.get().map(String::as_str), Some("fetched"));
    }

    #[tokio::test]
    async fn cached_public_docs_bundle_honors_longer_initial_wait() {
        let cell = Box::leak(Box::new(OnceCell::new()));
        let started = Box::leak(Box::new(AtomicBool::new(false)));

        let content = cached_public_docs_bundle(
            cell,
            started,
            "fallback",
            Duration::from_millis(PUBLIC_DOCS_INITIAL_WAIT_MILLIS * 3),
            || async {
                tokio::time::sleep(Duration::from_millis(PUBLIC_DOCS_INITIAL_WAIT_MILLIS * 2))
                    .await;
                "fetched".to_string()
            },
        )
        .await;

        assert_eq!(content, "fetched");
        assert_eq!(cell.get().map(String::as_str), Some("fetched"));
    }

    #[tokio::test]
    async fn cached_public_docs_bundle_waits_for_in_progress_fetch() {
        let cell = Box::leak(Box::new(OnceCell::new()));
        let started = Box::leak(Box::new(AtomicBool::new(false)));

        let first = cached_public_docs_bundle(
            cell,
            started,
            "fallback",
            Duration::from_millis(1),
            || async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                "fetched".to_string()
            },
        )
        .await;

        assert_eq!(first, "fallback");

        let second = cached_public_docs_bundle(
            cell,
            started,
            "fallback",
            Duration::from_millis(200),
            || async { "should not run".to_string() },
        )
        .await;

        assert_eq!(second, "fetched");
    }

    #[tokio::test]
    async fn cached_public_docs_bundle_zero_wait_does_not_start_fetch() {
        let cell = Box::leak(Box::new(OnceCell::new()));
        let started = Box::leak(Box::new(AtomicBool::new(false)));

        let content =
            cached_public_docs_bundle(cell, started, "fallback", Duration::ZERO, || async {
                "fetched".to_string()
            })
            .await;

        assert_eq!(content, "fallback");
        assert_eq!(cell.get(), None);
        assert!(!started.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_mcp_server_creation() {
        let server = CodemodMcpServer::default();
        let info = server.get_info();

        assert_eq!(info.protocol_version, ProtocolVersion::V_2024_11_05);
        assert!(info.capabilities.tools.is_some());
        assert!(info.instructions.is_some());
        let instructions = info.instructions.as_deref().unwrap_or_default();
        assert!(instructions.contains("validate_codemod_package"));
        assert!(!instructions.contains("scaffold_codemod_package"));
        assert!(instructions.contains("jssg-gotchas"));
        assert!(instructions.contains("codemod-creation-workflow-instructions"));
        assert!(instructions.contains("direct codemod init"));
        assert!(instructions.contains("auth-derived author"));
        assert!(instructions.contains("anonymous feedback"));
    }

    #[tokio::test]
    async fn test_instruction_alias_tool_returns_resource_content() {
        let server = CodemodMcpServer::default();
        let result = server
            .get_jssg_instructions(rmcp::handler::server::wrapper::Parameters(
                GetInstructionsRequest {},
            ))
            .await
            .expect("expected compatibility tool result");

        assert_eq!(result.is_error, Some(false));
        assert_eq!(result.content.len(), 1);
        let text = result.content[0]
            .as_text()
            .expect("expected text content from compatibility tool");
        assert!(text.text.contains("JSSG"));
    }

    #[tokio::test]
    async fn test_legacy_instruction_alias_tools_return_resource_content() {
        let server = CodemodMcpServer::default();

        let runtime = server
            .get_jssg_runtime_capabilities(rmcp::handler::server::wrapper::Parameters(
                GetInstructionsRequest {},
            ))
            .await
            .expect("expected runtime compatibility tool result");
        let troubleshooting = server
            .get_codemod_troubleshooting(rmcp::handler::server::wrapper::Parameters(
                GetInstructionsRequest {},
            ))
            .await
            .expect("expected troubleshooting compatibility tool result");
        let creation = server
            .get_codemod_creation_workflow(rmcp::handler::server::wrapper::Parameters(
                GetInstructionsRequest {},
            ))
            .await
            .expect("expected creation compatibility tool result");
        let monorepo = server
            .get_codemod_maintainer_monorepo(rmcp::handler::server::wrapper::Parameters(
                GetInstructionsRequest {},
            ))
            .await
            .expect("expected monorepo compatibility tool result");

        assert_eq!(runtime.is_error, Some(false));
        assert_eq!(troubleshooting.is_error, Some(false));
        assert_eq!(creation.is_error, Some(false));
        assert_eq!(monorepo.is_error, Some(false));

        assert!(runtime.content[0]
            .as_text()
            .expect("expected runtime text content")
            .text
            .contains("Canonical JSSG Runtime Capabilities Documentation"));
        assert!(troubleshooting.content[0]
            .as_text()
            .expect("expected troubleshooting text content")
            .text
            .contains("Troubleshooting"));
        assert!(creation.content[0]
            .as_text()
            .expect("expected creation text content")
            .text
            .contains("Codemod Creation"));
        assert!(monorepo.content[0]
            .as_text()
            .expect("expected monorepo text content")
            .text
            .contains("Canonical Codemod Maintainer Documentation"));
    }

    #[test]
    fn test_legacy_instruction_request_accepts_null_and_empty_object() {
        serde_json::from_value::<GetInstructionsRequest>(serde_json::Value::Null)
            .expect("expected null to deserialize");
        serde_json::from_value::<GetInstructionsRequest>(json!({}))
            .expect("expected empty object to deserialize");
    }

    #[tokio::test]
    async fn test_jssg_runtime_capabilities_resource_returns_prompt() {
        let server = CodemodMcpServer::default();
        let result = server
            .resource_content("jssg-runtime-capabilities://instructions")
            .await
            .expect("expected resource result");

        assert!(result.contains("Canonical JSSG Runtime Capabilities Documentation"));
        assert!(result.contains("jssgTransform"));
    }

    #[test]
    fn test_gotcha_resources_are_listed() {
        let server = CodemodMcpServer::default();
        let resources = server.resources();

        let resource_names = resources
            .iter()
            .map(|resource| resource.name.as_str())
            .collect::<Vec<_>>();
        assert!(resource_names.contains(&"jssg-gotchas"));
        assert!(resource_names.contains(&"ast-grep-gotchas"));
    }

    #[tokio::test]
    async fn test_gotcha_resources_are_readable() {
        let server = CodemodMcpServer::default();
        let jssg_gotchas = server
            .resource_content("jssg-gotchas://instructions")
            .await
            .expect("expected jssg gotchas");
        let ast_grep_gotchas = server
            .resource_content("ast-grep-gotchas://instructions")
            .await
            .expect("expected ast-grep gotchas");

        assert!(jssg_gotchas.contains("Canonical JSSG Gotchas Documentation"));
        assert!(ast_grep_gotchas.contains("Canonical ast-grep Usage Documentation"));
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
        server.log_usage("resource:jssg-instructions");

        let content = wait_for_usage_log(&temp_dir.join(relative_log_path));
        assert!(content.contains("resource:jssg-instructions"));

        drop(_cwd_guard);
        fs::remove_dir_all(&temp_dir).expect("expected temp dir cleanup");
    }

    #[test]
    fn feedback_event_names_are_sanitized() {
        assert_eq!(
            sanitize_feedback_event("cli:resource:jssg://instructions"),
            "cli:resource:jssg:__instructions"
        );
        assert_eq!(sanitize_feedback_event("  "), "unknown");
    }
}

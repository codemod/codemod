use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;
/// Represents a step in a node
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct Step {
    /// Unique identifier for the step (optional, used for referencing step outputs)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub id: Option<String>,

    /// Human-readable name
    pub name: String,

    /// Action to perform - either using a template or running a script
    #[serde(flatten)]
    pub action: StepAction,

    /// Environment variables specific to this step
    #[serde(default)]
    #[ts(optional, as = "Option<HashMap<String, String>>")]
    pub env: Option<HashMap<String, String>>,

    /// Conditional expression to determine if this step should be executed
    #[serde(rename = "if")]
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub condition: Option<String>,
}

/// Represents the action a step can take - either using templates or running a script
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum StepAction {
    /// Template to use for this step
    #[serde(rename = "use")]
    UseTemplate(TemplateUse),

    /// Script to run
    #[serde(rename = "run")]
    RunScript(String),

    /// ast-grep
    #[serde(rename = "ast-grep")]
    AstGrep(UseAstGrep),

    /// JavaScript AST grep execution
    #[serde(rename = "js-ast-grep")]
    JSAstGrep(UseJSAstGrep),

    /// Execute another codemod
    #[serde(rename = "codemod")]
    Codemod(UseCodemod),

    /// Execute AI agent with prompt
    #[serde(rename = "ai")]
    AI(UseAI),
}

/// Represents a template use in a step
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct TemplateUse {
    /// Template ID to use
    pub template: String,

    /// Inputs to pass to the template
    #[serde(default)]
    #[ts(optional, as = "Option<HashMap<String, serde_json::Value>>")]
    pub inputs: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub struct UseAstGrep {
    /// Include globs for files to search (optional, defaults to language-specific extensions)
    #[serde(default)]
    #[ts(optional, as = "Option<Vec<String>>")]
    pub include: Option<Vec<String>>,

    /// Exclude globs for files to skip (optional)
    #[serde(default)]
    #[ts(optional, as = "Option<Vec<String>>")]
    pub exclude: Option<Vec<String>>,

    /// Base path for resolving relative globs (optional, defaults to current working directory)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub base_path: Option<String>,

    /// Set maximum number of concurrent threads (optional, defaults to CPU cores)
    #[serde(default)]
    #[ts(optional, as = "Option<usize>")]
    pub max_threads: Option<usize>,

    /// Path to the ast-grep config file (.yaml)
    pub config_file: String,

    /// Allow dirty files (optional, defaults to false)
    #[serde(default)]
    #[ts(optional, as = "Option<bool>")]
    pub allow_dirty: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub struct UseJSAstGrep {
    /// Path to the JavaScript file to execute
    pub js_file: String,

    /// Include globs for files to search (optional, defaults to language-specific extensions)
    #[serde(default)]
    #[ts(optional, as = "Option<Vec<String>>")]
    pub include: Option<Vec<String>>,

    /// Exclude globs for files to skip (optional)
    #[serde(default)]
    #[ts(optional, as = "Option<Vec<String>>")]
    pub exclude: Option<Vec<String>>,

    /// Base path for resolving relative globs (optional, defaults to current working directory)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub base_path: Option<String>,

    /// Set maximum number of concurrent threads (optional, defaults to CPU cores)
    #[serde(default)]
    #[ts(optional, as = "Option<usize>")]
    pub max_threads: Option<usize>,

    /// Perform a dry run without making changes (optional, defaults to false)
    #[serde(default)]
    #[ts(optional, as = "Option<bool>")]
    pub dry_run: Option<bool>,

    /// Language to process (optional)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub language: Option<String>,

    /// Capabilities to use (optional)
    #[serde(default)]
    #[ts(optional, as = "Option<Vec<String>>")]
    pub capabilities: Option<Vec<String>>,

    /// Semantic analysis configuration for symbol indexing (getDefinition, findReferences).
    /// Can be:
    /// - `"file"` - single-file analysis (default if semantic is enabled)
    /// - `"workspace"` - workspace-wide analysis using base_path as root
    /// - `{"mode": "workspace", "root": "/path/to/workspace"}` - workspace-wide with custom root
    #[serde(default)]
    #[ts(optional, as = "Option<SemanticAnalysisConfig>")]
    pub semantic_analysis: Option<SemanticAnalysisConfig>,
}

/// Configuration for semantic analysis in JS AST grep.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum SemanticAnalysisConfig {
    /// Simple mode: "file" or "workspace"
    Mode(SemanticAnalysisMode),
    /// Detailed configuration with custom root path
    Detailed(SemanticAnalysisDetailed),
}

/// Simple semantic analysis mode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum SemanticAnalysisMode {
    /// Single-file analysis, no cross-file resolution
    File,
    /// Workspace-wide analysis with cross-file support
    Workspace,
}

/// Detailed semantic analysis configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct SemanticAnalysisDetailed {
    /// Analysis mode
    pub mode: SemanticAnalysisMode,
    /// Custom workspace root path (only used when mode is "workspace")
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub struct UseCodemod {
    /// Codemod source identifier (registry package or local path)
    pub source: String,

    /// Command line arguments to pass to the codemod (optional)
    #[serde(default)]
    #[ts(optional, as = "Option<Vec<String>>")]
    pub args: Option<Vec<String>>,

    /// Environment variables to set for the codemod execution (optional)
    #[serde(default)]
    #[ts(optional, as = "Option<HashMap<String, String>>")]
    pub env: Option<HashMap<String, String>>,

    /// Working directory for codemod execution (optional, defaults to current directory)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub struct UseAI {
    /// Prompt to send to the AI agent
    pub prompt: String,

    /// Working directory for AI agent execution (optional, defaults to current directory)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub working_dir: Option<String>,

    /// Timeout in milliseconds for AI agent execution (optional)
    #[serde(default)]
    #[ts(optional, as = "Option<u64>")]
    pub timeout_ms: Option<u64>,

    /// Environment variables to set for the AI agent execution (optional)
    #[serde(default)]
    #[ts(optional, as = "Option<HashMap<String, String>>")]
    pub env: Option<HashMap<String, String>>,

    /// Perform a dry run without making changes (optional, defaults to false)
    #[serde(default)]
    #[ts(optional, as = "Option<bool>")]
    pub dry_run: Option<bool>,

    /// AI model to use (optional, defaults to configured model)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub model: Option<String>,

    /// System prompt for the AI agent (optional)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub system_prompt: Option<String>,

    /// Maximum number of steps the AI agent can take (optional, defaults to 100)
    #[serde(default)]
    #[ts(optional, as = "Option<usize>")]
    pub max_steps: Option<usize>,

    /// Tools available to the AI agent (optional, defaults to common tools)
    #[serde(default)]
    #[ts(optional, as = "Option<Vec<String>>")]
    pub tools: Option<Vec<String>>,

    /// LLM API endpoint (optional, defaults to configured endpoint)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub endpoint: Option<String>,

    /// LLM API key (optional, defaults to configured key or env var)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub api_key: Option<String>,

    /// Enable lakeview mode (optional, defaults to true)
    #[serde(default)]
    #[ts(optional, as = "Option<bool>")]
    pub enable_lakeview: Option<bool>,

    /// LLM protocol to use (optional, defaults to openai)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub llm_protocol: Option<String>,
}

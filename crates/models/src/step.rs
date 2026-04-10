use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
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

    /// Conditional expression to determine if this step should be executed.
    /// Accepts a string expression or a literal boolean (`if: true` / `if: false`).
    #[serde(rename = "if")]
    #[serde(default, deserialize_with = "deserialize_condition")]
    #[ts(optional, as = "Option<String>")]
    pub condition: Option<String>,

    /// Optional commit checkpoint — if present (and in cloud mode), a git commit
    /// is created after this step completes. The message supports `${{ }}` expressions.
    #[serde(default)]
    #[ts(optional, as = "Option<CommitConfig>")]
    pub commit: Option<CommitConfig>,
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

    /// Install package skill behavior via codemod CLI
    #[serde(rename = "install-skill")]
    InstallSkill(UseInstallSkill),

    /// Evaluate file shards and write results to workflow state
    #[serde(rename = "shard")]
    Shard(UseShard),
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

    /// Semantic analysis configuration for symbol indexing (definition, references, typeInfo).
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

    /// Timeout in milliseconds for AI agent execution (optional)
    #[serde(default)]
    #[ts(optional, as = "Option<u64>")]
    pub timeout_ms: Option<u64>,

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub struct UseInstallSkill {
    /// Package identifier to install (for example `@codemod/jest-to-vitest`)
    pub package: String,

    /// Authored skill source path inside the package (optional, defaults to conventional layout)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub path: Option<String>,

    /// Target harness adapter (optional, defaults to auto)
    #[serde(default)]
    #[ts(optional, as = "Option<InstallSkillHarness>")]
    pub harness: Option<InstallSkillHarness>,

    /// Install scope (optional, defaults to project)
    #[serde(default)]
    #[ts(optional, as = "Option<InstallSkillScope>")]
    pub scope: Option<InstallSkillScope>,

    /// Overwrite existing skill files (optional, defaults to false)
    #[serde(default)]
    #[ts(optional, as = "Option<bool>")]
    pub force: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum InstallSkillHarness {
    Auto,
    Claude,
    Goose,
    Opencode,
    Cursor,
    Codex,
    Antigravity,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum InstallSkillScope {
    Project,
    User,
}

/// Configuration for the `shard` step action.
/// Evaluates file shards and writes results to workflow state.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub struct UseShard {
    /// Sharding method configuration — either a built-in algorithm or a custom function
    pub method: ShardMethod,

    /// Root directory to scan for files
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub target: Option<String>,

    /// State key to write shard results to
    pub output_state: String,

    /// Glob pattern for eligible files (used when js-ast-grep is not set)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub file_pattern: Option<String>,

    /// Optional js-ast-grep configuration for codemod-based pre-filtering.
    /// When set, the engine dry-runs the codemod against matched files and
    /// only includes files where the transform produces changes.
    /// The `include` field also serves as the file pattern for discovery.
    #[serde(rename = "js-ast-grep")]
    #[serde(default)]
    #[ts(optional, as = "Option<UseJSAstGrep>")]
    pub js_ast_grep: Option<UseJSAstGrep>,
}

/// Sharding method — either a built-in algorithm or a custom JS/TS function.
///
/// Deserialized from YAML as:
///   `{ type: directory, max_files_per_shard: 30 }`
///   `{ function: "./scripts/shard.ts" }`
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum ShardMethod {
    /// Built-in sharding method with sizing parameters
    Builtin(BuiltinShardMethod),
    /// Custom JS/TS shard function
    Function(CustomShardFunction),
}

/// Built-in sharding method configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub struct BuiltinShardMethod {
    /// Built-in method type
    pub r#type: BuiltinShardType,

    /// Target number of files per shard
    pub max_files_per_shard: usize,

    /// Minimum shard size — trailing shards smaller than this are merged
    /// into the previous shard (optional)
    #[serde(default)]
    #[ts(optional, as = "Option<usize>")]
    pub min_shard_size: Option<usize>,
}

/// Type of built-in sharding method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinShardType {
    /// Group by immediate subdirectory under target, then bin-pack
    Directory,
    /// Group by CODEOWNERS team, then bin-pack
    Codeowner,
}

/// Custom shard function configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub struct CustomShardFunction {
    /// Path to JS/TS shard function file
    pub function: String,
}

/// Configuration for a commit checkpoint on a step.
/// When present (and cloud mode is active), a git commit is created after the step runs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub struct CommitConfig {
    /// Commit message (supports `${{ }}` template expressions)
    pub message: String,

    /// Paths to stage before committing (default: `["."]` — stage everything)
    #[serde(default)]
    #[ts(optional, as = "Option<Vec<String>>")]
    pub add: Option<Vec<String>>,

    /// If true, skip silently when there are no changes to commit (default: true)
    #[serde(default = "default_allow_empty")]
    pub allow_empty: bool,
}

fn default_allow_empty() -> bool {
    true
}

/// Deserialize the `if` condition field from either a string or a boolean.
/// YAML `if: true` / `if: false` are parsed as booleans; this converts them
/// to the string literals `"true"` / `"false"` so the expression engine can
/// evaluate them uniformly.
fn deserialize_condition<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s)),
        Some(serde_json::Value::Bool(b)) => Ok(Some(b.to_string())),
        Some(serde_json::Value::Number(n)) => Ok(Some(n.to_string())),
        Some(other) => Err(serde::de::Error::custom(format!(
            "unsupported type for `if` condition: expected string or boolean, got {}",
            match &other {
                serde_json::Value::Array(_) => "array",
                serde_json::Value::Object(_) => "object",
                _ => "unknown",
            }
        ))),
    }
}

/// Configuration for automatic pull request creation at the end of a node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub struct PullRequestConfig {
    /// PR title (supports `${{ }}` template expressions)
    pub title: String,

    /// PR body/description (supports `${{ }}` template expressions)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub body: Option<String>,

    /// Create the pull request as a draft
    #[serde(default)]
    #[ts(optional, as = "Option<bool>")]
    pub draft: Option<bool>,

    /// Base branch to merge into (auto-detected if omitted)
    #[serde(default)]
    #[ts(optional, as = "Option<String>")]
    pub base: Option<String>,
}

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;
/// Represents a step in a node
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct Step {
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
}

/// Represents a template use in a step
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct TemplateUse {
    /// Template ID to use
    pub template: String,

    /// Inputs to pass to the template
    #[serde(default)]
    #[ts(optional, as = "Option<HashMap<String, String>>")]
    pub inputs: HashMap<String, String>,
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

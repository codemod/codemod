use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use codemod_llrt_capabilities::types::LlrtSupportedModules;

use crate::{
    ai_handoff::AgentOption,
    execution::{CodemodExecutionConfig, ProgressCallback},
    registry::RegistryClient,
    structured_log::OutputFormat,
};

pub type CapabilitiesSecurityCallback =
    Arc<Box<dyn Fn(&CodemodExecutionConfig) -> Result<(), anyhow::Error> + Send + Sync>>;
pub type PreRunCallback = Box<dyn Fn(&Path, bool) + Send + Sync>;

/// Callback for selecting an agent from available options.
/// Returns the canonical name of the selected agent, or None to skip.
pub type AgentSelectionCallback = Arc<Box<dyn Fn(&[AgentOption]) -> Option<String> + Send + Sync>>;

/// Info about a file that would be modified in dry-run mode
#[derive(Clone, Debug)]
pub struct DryRunChange {
    /// Path to the file that would be modified
    pub file_path: PathBuf,
    /// Original content of the file
    pub original_content: String,
    /// New content that would be written
    pub new_content: String,
}

/// Callback type for reporting dry-run changes
pub type DryRunCallback = Arc<Box<dyn Fn(DryRunChange) + Send + Sync>>;

/// Configuration for running a workflow
#[derive(Clone)]
pub struct WorkflowRunConfig {
    pub workflow_file_path: PathBuf,
    pub bundle_path: PathBuf,
    pub target_path: PathBuf,
    pub params: HashMap<String, serde_json::Value>,
    pub wait_for_completion: bool,
    pub progress_callback: Arc<Option<ProgressCallback>>,
    pub pre_run_callback: Arc<Option<PreRunCallback>>,
    pub registry_client: RegistryClient,
    pub dry_run: bool,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
    pub capabilities_security_callback: Option<CapabilitiesSecurityCallback>,
    /// Non-interactive mode for CI/headless environments
    pub no_interactive: bool,
    /// Explicitly selected coding agent for AI steps (e.g. "claude-code", "codex")
    pub agent: Option<String>,
    /// Callback for presenting agent selection UI when no agent is specified
    pub agent_selection_callback: Option<AgentSelectionCallback>,
    /// Callback for reporting changes in dry-run mode
    pub dry_run_callback: Option<DryRunCallback>,
    /// Output format for structured logging (Text or Jsonl)
    pub output_format: OutputFormat,
}

impl Default for WorkflowRunConfig {
    fn default() -> Self {
        Self {
            workflow_file_path: PathBuf::from("workflow.json"),
            bundle_path: PathBuf::from("bundle.json"),
            target_path: PathBuf::from("."),
            params: HashMap::new(),
            wait_for_completion: true,
            progress_callback: Arc::new(None),
            pre_run_callback: Arc::new(None),
            registry_client: RegistryClient::default(),
            dry_run: false,
            capabilities: None,
            capabilities_security_callback: None,
            no_interactive: false,
            agent: None,
            agent_selection_callback: None,
            dry_run_callback: None,
            output_format: OutputFormat::Text,
        }
    }
}

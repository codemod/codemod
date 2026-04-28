use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use async_trait::async_trait;
use butterflow_models::step::UseInstallSkill;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use thiserror::Error;

use crate::{
    ai_handoff::AgentOption,
    execution::{CodemodExecutionConfig, ProgressCallback},
    registry::RegistryClient,
    structured_log::OutputFormat,
};

pub type CapabilitiesSecurityCallback =
    Arc<dyn Fn(&CodemodExecutionConfig) -> Result<(), anyhow::Error> + Send + Sync>;
pub type PreRunCallback =
    Box<dyn Fn(&Path, bool, &WorkflowRunConfig) -> Result<(), anyhow::Error> + Send + Sync>;

/// Callback for selecting an agent from available options.
/// Returns the canonical name of the selected agent, or None to skip.
pub type AgentSelectionCallback = Arc<dyn Fn(&[AgentOption]) -> Option<String> + Send + Sync>;

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
pub type DryRunCallback = Arc<dyn Fn(DryRunChange) + Send + Sync>;

#[derive(Clone, Debug)]
pub struct SelectionPromptOption {
    pub value: String,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct SelectionPrompt {
    pub title: String,
    pub options: Vec<SelectionPromptOption>,
    pub default_index: usize,
}

pub type SelectionPromptCallback =
    Arc<dyn Fn(SelectionPrompt) -> Result<Option<String>, anyhow::Error> + Send + Sync>;

#[derive(Clone, Debug, Error)]
#[error("{message}")]
pub struct DeferredInteractionError {
    message: String,
}

impl DeferredInteractionError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Clone, Debug)]
pub struct ShellCommandExecutionRequest {
    pub command: String,
    pub node_id: String,
    pub node_name: String,
    pub step_id: Option<String>,
    pub step_name: String,
    pub task_id: String,
}

pub type ShellCommandApprovalCallback =
    Arc<dyn Fn(&ShellCommandExecutionRequest) -> Result<bool, anyhow::Error> + Send + Sync>;

#[derive(Clone, Debug)]
pub struct PullRequestCreationRequest {
    pub title: String,
    pub body: Option<String>,
    pub draft: bool,
    pub head: String,
    pub base: Option<String>,
    pub node_id: String,
    pub node_name: String,
    pub task_id: String,
}

pub type PullRequestApprovalCallback =
    Arc<dyn Fn(&PullRequestCreationRequest) -> Result<bool, anyhow::Error> + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirtyGitApprovalKind {
    UncommittedChanges,
    NotTracked,
}

#[derive(Clone, Debug)]
pub struct DirtyGitApprovalRequest {
    pub path: PathBuf,
    pub kind: DirtyGitApprovalKind,
}

pub type DirtyGitApprovalCallback =
    Arc<dyn Fn(&DirtyGitApprovalRequest) -> Result<bool, anyhow::Error> + Send + Sync>;

#[derive(Clone, Debug)]
pub struct ManagedGitWorktree {
    pub branch: String,
    pub path: PathBuf,
}

#[derive(Clone)]
pub struct InstallSkillExecutionRequest {
    pub install_skill: UseInstallSkill,
    pub no_interactive: bool,
    pub quiet: bool,
    pub bundle_path: Option<PathBuf>,
    pub target_path: PathBuf,
    pub env: HashMap<String, String>,
    pub output_format: OutputFormat,
    pub selection_prompt_callback: Option<SelectionPromptCallback>,
}

#[async_trait]
pub trait InstallSkillExecutor: Send + Sync {
    async fn execute(&self, request: InstallSkillExecutionRequest) -> Result<String>;
}

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
    pub selection_prompt_callback: Option<SelectionPromptCallback>,
    /// Callback for reporting changes in dry-run mode
    pub dry_run_callback: Option<DryRunCallback>,
    /// Skip executing install-skill steps at runtime (used by package run UX)
    pub skip_install_skill_steps: bool,
    /// Auto-trigger manual steps instead of waiting for user input (used by pro codemod dry-run)
    pub auto_trigger_manual_steps: bool,
    /// Skip shard steps entirely (used by pro codemod dry-run)
    pub skip_shard_steps: bool,
    /// Skip writing to workflow state (used by pro codemod dry-run)
    pub skip_state_writes: bool,
    /// Flatten matrix tasks to a single task per node (used by pro codemod dry-run)
    pub flatten_matrix_tasks: bool,
    /// Output format for structured logging (Text or Jsonl)
    pub output_format: OutputFormat,
    /// Human-readable name for this workflow run
    pub name: Option<String>,
    /// Suppress stdout/stderr output (used when TUI is active)
    pub quiet: bool,
    /// When quiet is enabled, capture stdout into task logs instead of letting it hit the terminal.
    /// TUI mode disables this so terminal rendering keeps control of stdout.
    pub capture_stdout_in_quiet_mode: bool,
    /// Optional interactive approval callback for shell-command workflow steps
    pub shell_command_approval_callback: Option<ShellCommandApprovalCallback>,
    /// Optional interactive approval callback for managed-git pull request creation
    pub pull_request_approval_callback: Option<PullRequestApprovalCallback>,
    /// Optional interactive approval callback for proceeding on dirty/untracked git targets
    pub dirty_git_approval_callback: Option<DirtyGitApprovalCallback>,
    /// Optional in-process executor for install-skill workflow steps
    pub install_skill_executor: Option<Arc<dyn InstallSkillExecutor>>,
    /// Optional per-task git worktree used for managed git execution.
    pub managed_git_worktree: Option<ManagedGitWorktree>,
    /// When false, skip managed git operations (branch switching, commits,
    /// push, pull request creation) for nodes that declare `branch_name` or
    /// `pull_request`. Cloud mode still manages git regardless of this flag.
    /// Defaults to true; CLI sets it to false for headless runs so those
    /// operations stay scoped to TUI-driven workflows.
    pub enable_managed_git: bool,
    /// When false, skip per-task git worktree creation even if managed git is
    /// enabled. Worktrees are only useful for TUI runs that execute multiple
    /// tasks in parallel against the same checkout. Cloud and headless CLI
    /// runs leave the working tree untouched and check out branches inline
    /// instead. Defaults to false.
    pub enable_worktrees: bool,
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
            selection_prompt_callback: None,
            dry_run_callback: None,
            skip_install_skill_steps: false,
            auto_trigger_manual_steps: false,
            skip_shard_steps: false,
            skip_state_writes: false,
            flatten_matrix_tasks: false,
            output_format: OutputFormat::Text,
            name: None,
            quiet: false,
            capture_stdout_in_quiet_mode: true,
            shell_command_approval_callback: None,
            pull_request_approval_callback: None,
            dirty_git_approval_callback: None,
            install_skill_executor: None,
            managed_git_worktree: None,
            enable_managed_git: true,
            enable_worktrees: false,
        }
    }
}

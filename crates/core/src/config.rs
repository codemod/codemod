//! Contracts between the CLI and the services in this crate: prompts the CLI
//! answers and executors it provides.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use butterflow_models::step::UseInstallSkill;
use thiserror::Error;

use crate::{execution::CodemodExecutionConfig, structured_log::OutputFormat};

pub type CapabilitiesSecurityCallback =
    Arc<dyn Fn(&CodemodExecutionConfig) -> Result<(), anyhow::Error> + Send + Sync>;

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

/// A question that cannot be answered now; the caller may ask again later.
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

/// Installs agent skills for `installSkill(...)` steps.
#[async_trait]
pub trait InstallSkillExecutor: Send + Sync {
    async fn execute(&self, request: InstallSkillExecutionRequest) -> Result<String>;
}

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    execution::{DownloadProgressCallback, ProgressCallback},
    registry::RegistryClient,
};
use anyhow::Result;
use codemod_llrt_capabilities::types::LlrtSupportedModules;

type DownloadProgressCallbackFn = Box<dyn Fn(u64, u64) + Send + Sync>;

#[derive(Clone)]
pub struct DownloadProgressCallback {
    pub callback: Arc<DownloadProgressCallbackFn>,
}

use crate::{
    execution::{CodemodExecutionConfig, ProgressCallback},
    registry::RegistryClient,
};

pub type CapabilitiesSecurityCallback =
    Arc<Box<dyn Fn(&CodemodExecutionConfig) -> Result<(), anyhow::Error> + Send + Sync>>;
pub type PreRunCallback = Box<dyn Fn(&Path, bool) + Send + Sync>;

/// Configuration for running a workflow
#[derive(Clone)]
pub struct WorkflowRunConfig {
    pub workflow_file_path: PathBuf,
    pub bundle_path: PathBuf,
    pub target_path: PathBuf,
    pub params: HashMap<String, serde_json::Value>,
    pub wait_for_completion: bool,
    pub progress_callback: Arc<Option<ProgressCallback>>,
    pub download_progress_callback: Option<DownloadProgressCallback>,
    pub pre_run_callback: Arc<Option<PreRunCallback>>,
    pub registry_client: RegistryClient,
    pub dry_run: bool,
    pub capabilities: Option<HashSet<LlrtSupportedModules>>,
    pub capabilities_security_callback: Option<CapabilitiesSecurityCallback>,
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
            download_progress_callback: None,
            pre_run_callback: Arc::new(None),
            registry_client: RegistryClient::default(),
            dry_run: false,
            capabilities: None,
            capabilities_security_callback: None,
        }
    }
}

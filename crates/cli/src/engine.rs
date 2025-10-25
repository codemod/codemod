use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use butterflow_core::config::{PreRunCallback, WorkflowRunConfig};
use butterflow_core::engine::Engine;
use butterflow_core::execution::ProgressCallback;
use butterflow_core::registry::{RegistryClient, RegistryConfig};
use butterflow_core::utils::get_cache_dir;
use butterflow_state::cloud_adapter::CloudStateAdapter;
use codemod_llrt_capabilities::types::LlrtSupportedModules;

use crate::auth_provider::CliAuthProvider;
use crate::capabilities_security_callback::capabilities_security_callback;
use crate::{dirty_git_check, progress_bar};

pub fn create_progress_callback() -> ProgressCallback {
    let (progress_reporter, _) = progress_bar::create_multi_progress_reporter();
    ProgressCallback {
        callback: Arc::new(Box::new(
            move |task_id: &str, path: &str, status: &str, count: Option<&u64>, index: &u64| {
                match status {
                    "start" | "counting" => {
                        progress_reporter(progress_bar::ProgressUpdate {
                            task_id: task_id.to_string(),
                            action: progress_bar::ProgressAction::Start {
                                total_files: count.cloned(),
                            },
                        });
                    }
                    "processing" => {
                        if !path.is_empty() {
                            progress_reporter(progress_bar::ProgressUpdate {
                                task_id: task_id.to_string(),
                                action: progress_bar::ProgressAction::Update {
                                    current_file: path.to_string(),
                                },
                            });
                        }
                    }
                    "increment" => {
                        progress_reporter(progress_bar::ProgressUpdate {
                            task_id: task_id.to_string(),
                            action: progress_bar::ProgressAction::Increment,
                        });
                    }
                    "finish" => {
                        let message = if let Some(total) = count {
                            format!("Processed {total} files")
                        } else {
                            format!("Processed {index} files")
                        };
                        progress_reporter(progress_bar::ProgressUpdate {
                            task_id: task_id.to_string(),
                            action: progress_bar::ProgressAction::Finish {
                                message: Some(message),
                            },
                        });
                    }
                    _ => {
                        // Handle any other status by updating current file
                        if !path.is_empty() {
                            progress_reporter(progress_bar::ProgressUpdate {
                                task_id: task_id.to_string(),
                                action: progress_bar::ProgressAction::Update {
                                    current_file: path.to_string(),
                                },
                            });
                        }
                    }
                }
            },
        )),
    }
}

/// Create an engine based on configuration
#[allow(clippy::too_many_arguments)]
pub fn create_engine(
    workflow_file_path: PathBuf,
    target_path: PathBuf,
    dry_run: bool,
    allow_dirty: bool,
    params: HashMap<String, serde_json::Value>,
    registry: Option<String>,
    capabilities: Option<HashSet<LlrtSupportedModules>>,
    no_interactive: bool,
) -> Result<(Engine, WorkflowRunConfig)> {
    let dirty_check = dirty_git_check::dirty_check();
    let bundle_path = if workflow_file_path.is_file() {
        workflow_file_path.parent().unwrap().to_path_buf()
    } else {
        workflow_file_path.to_path_buf()
    };

    let pre_run_callback: PreRunCallback = Box::new(move |path: &Path, dirty: bool| {
        if !allow_dirty {
            dirty_check(path, dirty);
        }
    });

    let progress_callback = create_progress_callback();

    let registry_client = create_registry_client(registry)?;

    let capabilities_security_callback = capabilities_security_callback(no_interactive);
    let config = WorkflowRunConfig {
        pre_run_callback: Arc::new(Some(pre_run_callback)),
        progress_callback: Arc::new(Some(progress_callback)),
        dry_run,
        target_path,
        workflow_file_path,
        bundle_path,
        params,
        registry_client,
        capabilities_security_callback: Some(capabilities_security_callback),
        capabilities,
        ..WorkflowRunConfig::default()
    };

    // Check for environment variables first
    if let (Some(backend), Some(endpoint), auth_token) = (
        std::env::var("BUTTERFLOW_STATE_BACKEND").ok(),
        std::env::var("BUTTERFLOW_API_ENDPOINT").ok(),
        std::env::var("BUTTERFLOW_API_AUTH_TOKEN")
            .ok()
            .unwrap_or_default(),
    ) {
        if backend == "cloud" {
            // Create API state adapter
            let state_adapter = Box::new(CloudStateAdapter::new(endpoint, auth_token));
            return Ok((
                Engine::with_state_adapter(state_adapter, config.clone()),
                config.clone(),
            ));
        }
    }

    Ok((Engine::with_workflow_run_config(config.clone()), config))
}

pub fn create_registry_client(registry: Option<String>) -> Result<RegistryClient> {
    // Create auth provider
    let auth_provider = CliAuthProvider::new()?;

    // Get cache directory and default registry from config
    let config = auth_provider.storage.load_config()?;

    let registry_url = registry.unwrap_or(config.default_registry);

    // Create registry configuration
    let registry_config = RegistryConfig {
        default_registry: registry_url.clone(),
        cache_dir: get_cache_dir().unwrap(),
    };

    Ok(RegistryClient::new(
        registry_config,
        Some(Arc::new(auth_provider)),
    ))
}

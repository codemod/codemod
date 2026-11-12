//! Registry client construction for commands that resolve packages.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use butterflow_core::execution::ProgressCallback;
use butterflow_core::registry::{RegistryClient, RegistryConfig};
use butterflow_core::utils::get_cache_dir;

use crate::auth_provider::CliAuthProvider;
use crate::utils::env_paths::data_dir_from_env;

pub fn create_registry_client(registry: Option<String>) -> Result<RegistryClient> {
    create_registry_client_with_env(registry, None)
}

pub fn create_registry_client_with_env(
    registry: Option<String>,
    env: Option<&HashMap<String, String>>,
) -> Result<RegistryClient> {
    let storage = crate::auth::TokenStorage::new()?;
    let auth_provider = CliAuthProvider::from_storage(storage);

    let config = auth_provider.storage.load_config_with_env(env)?;
    let registry_url = registry.unwrap_or(config.default_registry);
    let cache_dir = get_cache_dir_with_env(env)?;

    let registry_config = RegistryConfig {
        default_registry: registry_url.clone(),
        cache_dir,
    };

    Ok(RegistryClient::new(
        registry_config,
        Some(Arc::new(auth_provider)),
    ))
}

fn get_cache_dir_with_env(env: Option<&HashMap<String, String>>) -> Result<PathBuf> {
    if let Some(data_dir) = data_dir_from_env(env) {
        return Ok(data_dir.join("codemod").join("cache").join("packages"));
    }

    Ok(get_cache_dir()?)
}

/// Progress bars for `codemod jssg run`.
pub fn create_progress_callback() -> ProgressCallback {
    let (progress_reporter, _) = crate::progress_bar::create_multi_progress_reporter();
    ProgressCallback {
        callback: Arc::new(Box::new(
            move |task_id: &str, path: &str, status: &str, count: Option<&u64>, index: &u64| {
                let nested_label = task_id
                    .split_once(':')
                    .map(|(_, label)| label.trim())
                    .filter(|label| !label.is_empty());
                match status {
                    "start" | "counting" => {
                        progress_reporter(crate::progress_bar::ProgressUpdate {
                            task_id: task_id.to_string(),
                            action: crate::progress_bar::ProgressAction::Start {
                                total_files: count.cloned(),
                                label: nested_label.map(str::to_string),
                            },
                        });
                    }
                    "processing" => {
                        if !path.is_empty() {
                            progress_reporter(crate::progress_bar::ProgressUpdate {
                                task_id: task_id.to_string(),
                                action: crate::progress_bar::ProgressAction::Update {
                                    current_file: path.to_string(),
                                },
                            });
                        }
                    }
                    "log" => {
                        if let Some((title, line)) = path.split_once('\n') {
                            progress_reporter(crate::progress_bar::ProgressUpdate {
                                task_id: task_id.to_string(),
                                action: crate::progress_bar::ProgressAction::Log {
                                    title: title.to_string(),
                                    line: line.to_string(),
                                },
                            });
                        }
                    }
                    "agent" => {
                        progress_reporter(crate::progress_bar::ProgressUpdate {
                            task_id: task_id.to_string(),
                            action: crate::progress_bar::ProgressAction::Agent {
                                payload: path.to_string(),
                            },
                        });
                    }
                    "diagnostic" => {
                        if let Some((title, message)) = path.split_once('\n') {
                            progress_reporter(crate::progress_bar::ProgressUpdate {
                                task_id: task_id.to_string(),
                                action: crate::progress_bar::ProgressAction::Diagnostic {
                                    title: title.to_string(),
                                    message: message.to_string(),
                                },
                            });
                        }
                    }
                    "increment" => {
                        progress_reporter(crate::progress_bar::ProgressUpdate {
                            task_id: task_id.to_string(),
                            action: crate::progress_bar::ProgressAction::Increment,
                        });
                    }
                    "finish" => {
                        let processed_message = if let Some(total) = count {
                            if *total == 1 {
                                "Processed 1 file".to_string()
                            } else {
                                "Processed files".to_string()
                            }
                        } else {
                            format!("Processed {index} files")
                        };
                        let message = nested_label
                            .map(|label| format!("{label}: {processed_message}"))
                            .unwrap_or(processed_message);
                        progress_reporter(crate::progress_bar::ProgressUpdate {
                            task_id: task_id.to_string(),
                            action: crate::progress_bar::ProgressAction::Finish {
                                message: Some(message),
                            },
                        });
                    }
                    _ => {
                        // Handle any other status by updating current file
                        if !path.is_empty() {
                            progress_reporter(crate::progress_bar::ProgressUpdate {
                                task_id: task_id.to_string(),
                                action: crate::progress_bar::ProgressAction::Update {
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

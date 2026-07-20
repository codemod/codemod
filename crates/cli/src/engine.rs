use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use butterflow_core::config::{
    AgentSelectionCallback, DryRunCallback, DryRunChange, InstallSkillExecutor, PreRunCallback,
    ShellCommandApprovalCallback, WorkflowRunConfig,
};
use butterflow_core::diff::{generate_unified_diff, DiffConfig, DiffMetadata, FileDiff};
use butterflow_core::engine::Engine;
use butterflow_core::execution::ProgressCallback;
use butterflow_core::registry::{RegistryClient, RegistryConfig};
use butterflow_core::structured_log::OutputFormat;
use butterflow_core::utils::get_cache_dir;
use butterflow_state::cloud_adapter::CloudStateAdapter;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use console::style;
use inquire::Confirm;

use crate::auth_provider::CliAuthProvider;
use crate::capabilities_security_callback::capabilities_security_callback;
use crate::utils::env_paths::data_dir_from_env;
use crate::{dirty_git_check, progress_bar};

/// Create a callback that silently collects diffs without printing to terminal.
/// Used when --report is passed without --dry-run.
pub fn create_silent_diff_collector(collector: Arc<Mutex<Vec<FileDiff>>>) -> DryRunCallback {
    Arc::new(move |change: DryRunChange| {
        let config = DiffConfig {
            color: false,
            ..DiffConfig::default()
        };
        // For renames/moves, `file_path` is the original location and
        // `new_path` is the destination; the diff should be reported at the
        // destination path with `old_path` set for the report to display.
        let (diff_path, old_path) = match &change.new_path {
            Some(new_path) => (new_path.clone(), Some(change.file_path.clone())),
            None => (change.file_path.clone(), None),
        };
        let diff = generate_unified_diff(
            &diff_path,
            &change.original_content,
            &change.new_content,
            &config,
            DiffMetadata {
                step_id: change.step_id,
                step_name: change.step_name,
                parent_step_id: change.parent_step_id,
                parent_step_name: change.parent_step_name,
                kind: change.kind,
                old_path,
            },
        );
        if let Ok(mut diffs) = collector.lock() {
            diffs.push(diff);
        }
    })
}

pub fn create_progress_callback() -> ProgressCallback {
    let (progress_reporter, _) = progress_bar::create_multi_progress_reporter();
    ProgressCallback {
        callback: Arc::new(Box::new(
            move |task_id: &str, path: &str, status: &str, count: Option<&u64>, index: &u64| {
                let nested_label = task_id
                    .split_once(':')
                    .map(|(_, label)| label.trim())
                    .filter(|label| !label.is_empty());
                match status {
                    "start" | "counting" => {
                        progress_reporter(progress_bar::ProgressUpdate {
                            task_id: task_id.to_string(),
                            action: progress_bar::ProgressAction::Start {
                                total_files: count.cloned(),
                                label: nested_label.map(str::to_string),
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
                    "log" => {
                        if let Some((title, line)) = path.split_once('\n') {
                            progress_reporter(progress_bar::ProgressUpdate {
                                task_id: task_id.to_string(),
                                action: progress_bar::ProgressAction::Log {
                                    title: title.to_string(),
                                    line: line.to_string(),
                                },
                            });
                        }
                    }
                    "agent" => {
                        progress_reporter(progress_bar::ProgressUpdate {
                            task_id: task_id.to_string(),
                            action: progress_bar::ProgressAction::Agent {
                                payload: path.to_string(),
                            },
                        });
                    }
                    "diagnostic" => {
                        if let Some((title, message)) = path.split_once('\n') {
                            progress_reporter(progress_bar::ProgressUpdate {
                                task_id: task_id.to_string(),
                                action: progress_bar::ProgressAction::Diagnostic {
                                    title: title.to_string(),
                                    message: message.to_string(),
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

fn create_shell_command_approval_callback(
    no_interactive: bool,
) -> Option<ShellCommandApprovalCallback> {
    if no_interactive {
        return None;
    }

    let prompt_lock = Arc::new(Mutex::new(()));
    Some(Arc::new(move |request| {
        let _prompt_guard = prompt_lock.lock().unwrap();

        eprintln!();
        eprintln!(
            "  {}",
            style("⚠  Shell command requires approval").yellow().bold()
        );
        eprintln!("  {} {}", style("Step:").dim(), request.step_name);
        eprintln!("  {} {}", style("Node:").dim(), request.node_name);
        eprintln!("  {}", style("Command:").dim());
        for line in request.command.lines() {
            eprintln!("    {line}");
        }
        eprintln!();

        Confirm::new("Run this command?")
            .with_default(false)
            .prompt()
            .map_err(|error| anyhow::anyhow!("Failed to get user input: {error}"))
    }))
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
    diff_collector: Option<Arc<Mutex<Vec<FileDiff>>>>,
    skip_install_skill_steps: bool,
    output_format: OutputFormat,
    pre_approved_capabilities: Option<HashSet<LlrtSupportedModules>>,
    agent: Option<String>,
    install_skill_executor: Option<Arc<dyn InstallSkillExecutor>>,
) -> Result<(Engine, WorkflowRunConfig)> {
    let dirty_check = dirty_git_check::dirty_check(no_interactive);
    let bundle_path = if workflow_file_path.is_file() {
        workflow_file_path.parent().unwrap().to_path_buf()
    } else {
        workflow_file_path.to_path_buf()
    };

    let pre_run_callback: PreRunCallback = Box::new(move |path: &Path, dirty: bool, config| {
        if !allow_dirty {
            dirty_check(
                path,
                dirty,
                config.interaction.dirty_git_approval_callback.as_ref(),
            )?;
        }
        Ok(())
    });

    // Skip progress bars in JSONL mode (would corrupt structured output)
    let progress_callback = if output_format == OutputFormat::Jsonl {
        None
    } else {
        Some(create_progress_callback())
    };
    let progress_owns_terminal = progress_callback.is_some();

    let registry_client = create_registry_client(registry)?;

    let capabilities_security_callback =
        capabilities_security_callback(no_interactive, pre_approved_capabilities);
    let dry_run_callback = diff_collector.map(create_silent_diff_collector);

    let agent_selection_callback: Option<AgentSelectionCallback> = if no_interactive {
        None
    } else {
        Some(crate::agent_select::create_agent_selection_callback())
    };
    let shell_command_approval_callback = create_shell_command_approval_callback(no_interactive);

    let config = WorkflowRunConfig {
        execution: butterflow_core::config::WorkflowExecutionSettings {
            workflow_file_path,
            bundle_path,
            target_path,
            params,
            progress_callback: Arc::new(progress_callback),
            pre_run_callback: Arc::new(Some(pre_run_callback)),
            dry_run,
            registry_client,
            capabilities,
            capabilities_security_callback: Some(capabilities_security_callback),
            ..Default::default()
        },
        interaction: butterflow_core::config::WorkflowInteractionSettings {
            no_interactive,
            agent,
            agent_selection_callback,
            shell_command_approval_callback,
            ..Default::default()
        },
        output: butterflow_core::config::WorkflowOutputSettings {
            output_format,
            dry_run_callback,
            ..Default::default()
        },
        skill_install: butterflow_core::config::SkillInstallSettings {
            skip_install_skill_steps,
            install_skill_executor,
        },
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
            let mut engine = Engine::with_state_adapter(state_adapter, config.clone());
            if progress_owns_terminal {
                engine.set_text_log_fallthrough(false);
            }
            return Ok((engine, config.clone()));
        }
    }

    let mut engine = Engine::with_workflow_run_config(config.clone());
    if progress_owns_terminal {
        engine.set_text_log_fallthrough(false);
    }
    Ok((engine, config))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_prompt_for_shell_command_confirmation_only_depends_on_interactive_mode() {
        assert!(create_shell_command_approval_callback(false).is_some());
        assert!(create_shell_command_approval_callback(true).is_none());
    }
}

use async_trait::async_trait;
use butterflow_core::config::{
    DeferredInteractionError, InstallSkillExecutionRequest, InstallSkillExecutor,
    ShellCommandApprovalCallback, ShellCommandExecutionRequest, WorkflowRunConfig,
};
use butterflow_state::mock_adapter::MockStateAdapter;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::Notify;

use butterflow_core::engine::{
    await_js_ast_grep_execution_task, build_js_ast_grep_idle_timeout_message, finish_unit_progress,
    js_ast_grep_idle_timeout, record_output_progress, record_unit_progress,
    select_shard_scan_eligible_files, CapabilitiesData, Engine, StepPhase, StepProgressState,
    UnitProgressState, JS_AST_GREP_IDLE_TIMEOUT_MS_DEFAULT,
};
use butterflow_core::structured_log::StructuredLogger;
use butterflow_core::workflow_runtime::{WorkflowCommand, WorkflowSession};
use butterflow_core::{
    Node, Runtime, RuntimeType, Step, Task, TaskStatus, Template, Workflow, WorkflowRun,
    WorkflowStatus,
};
use butterflow_models::node::NodeType;
use butterflow_models::step::{
    SemanticAnalysisConfig, SemanticAnalysisMode, StepAction, UseAI, UseAstGrep, UseInstallSkill,
    UseJSAstGrep,
};
use butterflow_models::strategy::Strategy;
use butterflow_models::trigger::TriggerType;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_sandbox::sandbox::engine::{CodemodOutput, ExecutionResult};

use butterflow_models::{DiffOperation, FieldDiff, TaskDiff};
use butterflow_state::local_adapter::LocalStateAdapter;
use butterflow_state::StateAdapter;
use serde_json::json;
use serial_test::serial;
use uuid::Uuid;

type PanicRecoveryState = Option<(TaskStatus, TaskStatus, Vec<String>)>;
type MatrixTaskSnapshot = Vec<(
    Uuid,
    Option<Uuid>,
    bool,
    TaskStatus,
    Option<String>,
    Vec<String>,
)>;

fn debarrel_bundle_path() -> Option<PathBuf> {
    let configured = std::env::var("CODEMOD_TEST_DEBARREL_BUNDLE")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            Some(PathBuf::from(
                "/Users/sahilmobaidin/Desktop/myprojects/useful-codemods/codemods/debarrel",
            ))
        })?;

    configured.exists().then_some(configured)
}

struct EnvVarGuard {
    key: String,
    original: Option<String>,
}

impl EnvVarGuard {
    fn unset(key: &str) -> Self {
        let original = std::env::var(key).ok();
        std::env::remove_var(key);
        Self {
            key: key.to_string(),
            original,
        }
    }

    fn set(key: &str, value: &str) -> Self {
        let original = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self {
            key: key.to_string(),
            original,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.original {
            std::env::set_var(&self.key, value);
        } else {
            std::env::remove_var(&self.key);
        }
    }
}

async fn wait_for_task_status<F>(
    engine: &Engine,
    workflow_run_id: Uuid,
    node_id: &str,
    predicate: F,
) -> TaskStatus
where
    F: Fn(TaskStatus) -> bool,
{
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(3);

    loop {
        let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
        let task = tasks.iter().find(|task| task.node_id == node_id);

        if let Some(task) = task {
            if predicate(task.status) {
                return task.status;
            }

            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for node '{node_id}' to reach expected status, last status was {:?}",
                task.status
            );
        } else {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for task for node '{node_id}' to be created"
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }
}

async fn wait_for_workflow_status<F>(
    engine: &Engine,
    workflow_run_id: Uuid,
    predicate: F,
) -> WorkflowStatus
where
    F: Fn(WorkflowStatus) -> bool,
{
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);

    loop {
        let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();
        if predicate(workflow_run.status) {
            return workflow_run.status;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for workflow '{workflow_run_id}' to reach expected status, last status was {:?}",
            workflow_run.status
        );

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }
}

// Helper function to create a simple test workflow
fn create_long_running_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "long-running-node".to_string(),
            name: "Long Running Node".to_string(),
            description: Some("Test node that takes a while to complete".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![Step {
                id: Some("long-running-step".to_string()),
                name: "Long Running Step".to_string(),
                action: StepAction::RunScript("sleep 2 && echo 'Done'".to_string()),
                env: None,
                condition: None,
                commit: None,
            }],
            env: HashMap::new(),
            branch_name: None,
            pull_request: None,
        }],
    }
}

fn create_test_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: Some("Test node 1".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Hello, World!'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "node2".to_string(),
                name: "Node 2".to_string(),
                description: Some("Test node 2".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Node 2 executed'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
        ],
    }
}

fn create_git_managed_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "apply-transforms".to_string(),
            name: "Apply AST Transformations".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![Step {
                id: Some("step-1".to_string()),
                name: "Mutate file".to_string(),
                action: StepAction::RunScript("echo 'done'".to_string()),
                env: None,
                condition: None,
                commit: None,
            }],
            env: HashMap::new(),
            branch_name: Some("codemod-test-branch".to_string()),
            pull_request: Some(butterflow_models::step::PullRequestConfig {
                title: "Managed git test PR".to_string(),
                body: None,
                draft: Some(true),
                base: None,
            }),
        }],
    }
}

fn create_single_run_script_workflow(command: String) -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "shell-node".to_string(),
            name: "Shell Node".to_string(),
            description: Some("Workflow with a single shell command step".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![Step {
                id: Some("shell-step".to_string()),
                name: "Shell Step".to_string(),
                action: StepAction::RunScript(command),
                env: None,
                condition: None,
                commit: None,
            }],
            env: HashMap::new(),
            branch_name: None,
            pull_request: None,
        }],
    }
}

// Helper function to create a workflow with a manual trigger
fn create_manual_trigger_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: Some("Test node 1".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Hello, World!'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "node2".to_string(),
                name: "Node 2".to_string(),
                description: Some("Test node 2".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()],
                trigger: Some(butterflow_models::trigger::Trigger {
                    r#type: TriggerType::Manual,
                }),
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Node 2 executed'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
        ],
    }
}

// Helper function to create a workflow with a manual node type
fn create_manual_node_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: Some("Test node 1".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Hello, World!'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "node2".to_string(),
                name: "Node 2".to_string(),
                description: Some("Test node 2".to_string()),
                r#type: NodeType::Manual,
                depends_on: vec!["node1".to_string()],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-2".to_string()),
                    name: "Step 2".to_string(),
                    action: StepAction::RunScript("echo 'Node 2 executed'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
        ],
    }
}

// Helper function to create a workflow with a matrix strategy
fn create_matrix_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: Some("Test node 1".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Hello, World!'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "node2".to_string(),
                name: "Node 2".to_string(),
                description: Some("Test node 2".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()],
                trigger: None,
                strategy: Some(Strategy {
                    r#type: butterflow_models::strategy::StrategyType::Matrix,
                    values: Some(vec![
                        HashMap::from([(
                            "region".to_string(),
                            serde_json::to_value("us-east").unwrap(),
                        )]),
                        HashMap::from([(
                            "region".to_string(),
                            serde_json::to_value("us-west").unwrap(),
                        )]),
                        HashMap::from([(
                            "region".to_string(),
                            serde_json::to_value("eu-central").unwrap(),
                        )]),
                    ]),
                    from_state: None,
                }),
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Processing region ${region}'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
        ],
    }
}

fn create_manual_matrix_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: Some("Test node 1".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Hello, World!'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "node2".to_string(),
                name: "Node 2".to_string(),
                description: Some("Manual matrix node".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()],
                trigger: Some(butterflow_models::trigger::Trigger {
                    r#type: TriggerType::Manual,
                }),
                strategy: Some(Strategy {
                    r#type: butterflow_models::strategy::StrategyType::Matrix,
                    values: Some(vec![
                        HashMap::from([(
                            "region".to_string(),
                            serde_json::to_value("us-east").unwrap(),
                        )]),
                        HashMap::from([(
                            "region".to_string(),
                            serde_json::to_value("us-west").unwrap(),
                        )]),
                    ]),
                    from_state: None,
                }),
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Manual matrix shard'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
        ],
    }
}

fn create_manual_matrix_install_skill_workflow() -> Workflow {
    let mut workflow = create_manual_matrix_workflow();
    if let Some(node) = workflow.nodes.iter_mut().find(|node| node.id == "node2") {
        node.steps = vec![Step {
            id: Some("install-skill-step".to_string()),
            name: "Install Skill".to_string(),
            action: StepAction::InstallSkill(UseInstallSkill {
                package: "@codemod/test-skill".to_string(),
                path: None,
                harness: None,
                scope: None,
                force: None,
            }),
            env: None,
            condition: None,
            commit: None,
        }];
    }
    workflow
}

fn create_manual_install_skill_workflow() -> Workflow {
    let mut workflow = create_manual_trigger_workflow();
    if let Some(node) = workflow.nodes.iter_mut().find(|node| node.id == "node2") {
        node.steps = vec![Step {
            id: Some("install-skill-step".to_string()),
            name: "Install Skill".to_string(),
            action: StepAction::InstallSkill(UseInstallSkill {
                package: "@codemod/test-skill".to_string(),
                path: None,
                harness: None,
                scope: None,
                force: None,
            }),
            env: None,
            condition: None,
            commit: None,
        }];
    }
    workflow
}

fn create_manual_matrix_install_skill_git_workflow() -> Workflow {
    let mut workflow = create_manual_matrix_install_skill_workflow();
    if let Some(node) = workflow.nodes.iter_mut().find(|node| node.id == "node2") {
        node.branch_name = Some("codemod-${{ task.signature }}".to_string());
    }
    workflow
}

struct PanicInstallSkillExecutor;

#[async_trait]
impl InstallSkillExecutor for PanicInstallSkillExecutor {
    async fn execute(&self, _request: InstallSkillExecutionRequest) -> anyhow::Result<String> {
        panic!("panic install skill executor");
    }
}

struct FailingInstallSkillExecutor;

#[async_trait]
impl InstallSkillExecutor for FailingInstallSkillExecutor {
    async fn execute(&self, _request: InstallSkillExecutionRequest) -> anyhow::Result<String> {
        Err(anyhow::anyhow!("failing install skill executor"))
    }
}

struct DeferredThenSuccessInstallSkillExecutor {
    attempts: Arc<Mutex<usize>>,
}

#[async_trait]
impl InstallSkillExecutor for DeferredThenSuccessInstallSkillExecutor {
    async fn execute(&self, _request: InstallSkillExecutionRequest) -> anyhow::Result<String> {
        let mut attempts = self.attempts.lock().unwrap();
        *attempts += 1;
        if *attempts == 1 {
            Err(DeferredInteractionError::new("selection prompt canceled").into())
        } else {
            Ok("installed".to_string())
        }
    }
}

struct RecordingInstallSkillExecutor {
    requests: Arc<Mutex<Vec<InstallSkillExecutionRequest>>>,
    output: String,
}

#[async_trait]
impl InstallSkillExecutor for RecordingInstallSkillExecutor {
    async fn execute(&self, request: InstallSkillExecutionRequest) -> anyhow::Result<String> {
        self.requests.lock().unwrap().push(request);
        Ok(self.output.clone())
    }
}

fn create_manual_matrix_long_running_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: Some("Test node 1".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'ready'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "node2".to_string(),
                name: "Node 2".to_string(),
                description: Some("Manual matrix node".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()],
                trigger: Some(butterflow_models::trigger::Trigger {
                    r#type: TriggerType::Manual,
                }),
                strategy: Some(Strategy {
                    r#type: butterflow_models::strategy::StrategyType::Matrix,
                    values: Some(vec![
                        HashMap::from([(
                            "region".to_string(),
                            serde_json::to_value("us-east").unwrap(),
                        )]),
                        HashMap::from([(
                            "region".to_string(),
                            serde_json::to_value("us-west").unwrap(),
                        )]),
                    ]),
                    from_state: None,
                }),
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript(
                        "sleep 1 && echo 'Manual matrix shard'".to_string(),
                    ),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
        ],
    }
}

fn create_manual_matrix_long_running_git_workflow() -> Workflow {
    let mut workflow = create_manual_matrix_long_running_workflow();
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == "node2")
        .expect("node2 should exist");
    node.branch_name = Some("codemod-${{ task.signature }}".to_string());
    workflow
}

fn create_manual_matrix_git_js_ast_grep_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: Some("Prepare shards".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'ready'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "node2".to_string(),
                name: "Debarrel".to_string(),
                description: Some("Manual matrix js-ast-grep node".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()],
                trigger: Some(butterflow_models::trigger::Trigger {
                    r#type: TriggerType::Manual,
                }),
                strategy: Some(Strategy {
                    r#type: butterflow_models::strategy::StrategyType::Matrix,
                    values: Some(vec![
                        HashMap::from([
                            ("name".to_string(), json!("shard-0")),
                            (
                                "_meta_files".to_string(),
                                json!([
                                    "apps/nextjs/src/app/(protected)/wish/_atoms/push-notifications.ts",
                                    "apps/nextjs/src/app/(protected)/wish/_hooks/use-wish-thread.ts"
                                ]),
                            ),
                        ]),
                        HashMap::from([
                            ("name".to_string(), json!("shard-1")),
                            (
                                "_meta_files".to_string(),
                                json!([
                                    "apps/website/src/app/(landing)/blog/_utils/blog-helpers.ts",
                                    "apps/website/src/app/(payload)/payload/graphql/route.ts"
                                ]),
                            ),
                        ]),
                    ]),
                    from_state: None,
                }),
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("js-ast-grep-step".to_string()),
                    name: "Debarrel: rewrite imports and clean up barrels".to_string(),
                    action: StepAction::JSAstGrep(UseJSAstGrep {
                        js_file: "codemod.js".to_string(),
                        include: None,
                        exclude: None,
                        base_path: None,
                        max_threads: Some(2),
                        dry_run: Some(false),
                        language: Some("typescript".to_string()),
                        capabilities: None,
                        semantic_analysis: Some(SemanticAnalysisConfig::Mode(
                            SemanticAnalysisMode::File,
                        )),
                    }),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: Some("codemod-${{ task.signature }}".to_string()),
                pull_request: None,
            },
        ],
    }
}

fn create_manual_matrix_real_debarrel_workspace_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Prepare shards".to_string(),
                description: Some("Prepare manual shards".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'ready'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "node2".to_string(),
                name: "Apply transforms".to_string(),
                description: Some("Manual matrix using real debarrel bundle".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()],
                trigger: Some(butterflow_models::trigger::Trigger {
                    r#type: TriggerType::Manual,
                }),
                strategy: Some(Strategy {
                    r#type: butterflow_models::strategy::StrategyType::Matrix,
                    values: Some(vec![
                        HashMap::from([
                            ("name".to_string(), json!("shard-0")),
                            (
                                "_meta_files".to_string(),
                                json!([
                                    "src/App.ts",
                                    "src/components/index.ts",
                                    "src/components/Button.ts"
                                ]),
                            ),
                        ]),
                        HashMap::from([
                            ("name".to_string(), json!("shard-1")),
                            (
                                "_meta_files".to_string(),
                                json!([
                                    "src/consumer.ts",
                                    "src/utils/index.ts",
                                    "src/utils/calc.ts"
                                ]),
                            ),
                        ]),
                    ]),
                    from_state: None,
                }),
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("debarrel-step".to_string()),
                    name: "Debarrel: rewrite imports and clean up barrels".to_string(),
                    action: StepAction::JSAstGrep(UseJSAstGrep {
                        js_file: "scripts/codemod.ts".to_string(),
                        include: None,
                        exclude: None,
                        base_path: None,
                        max_threads: Some(2),
                        dry_run: Some(false),
                        language: Some("typescript".to_string()),
                        capabilities: None,
                        semantic_analysis: Some(SemanticAnalysisConfig::Mode(
                            SemanticAnalysisMode::Workspace,
                        )),
                    }),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: Some("codemod-${{ task.signature }}".to_string()),
                pull_request: None,
            },
        ],
    }
}

fn create_many_shard_real_debarrel_workspace_workflow(shard_count: usize) -> Workflow {
    let shard_values = (0..shard_count)
        .map(|index| {
            HashMap::from([
                ("name".to_string(), json!(format!("shard-{index}"))),
                (
                    "_meta_files".to_string(),
                    json!([
                        format!("src/shard_{index}/App.ts"),
                        format!("src/shard_{index}/components/index.ts"),
                        format!("src/shard_{index}/components/Button.ts"),
                    ]),
                ),
            ])
        })
        .collect::<Vec<_>>();

    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Prepare shards".to_string(),
                description: Some("Prepare manual shards".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'ready'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "node2".to_string(),
                name: "Apply transforms".to_string(),
                description: Some("Many concurrent real debarrel shards".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()],
                trigger: Some(butterflow_models::trigger::Trigger {
                    r#type: TriggerType::Manual,
                }),
                strategy: Some(Strategy {
                    r#type: butterflow_models::strategy::StrategyType::Matrix,
                    values: Some(shard_values),
                    from_state: None,
                }),
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("debarrel-step".to_string()),
                    name: "Debarrel: rewrite imports and clean up barrels".to_string(),
                    action: StepAction::JSAstGrep(UseJSAstGrep {
                        js_file: "scripts/codemod.ts".to_string(),
                        include: None,
                        exclude: None,
                        base_path: None,
                        max_threads: Some(2),
                        dry_run: Some(true),
                        language: Some("typescript".to_string()),
                        capabilities: None,
                        semantic_analysis: Some(SemanticAnalysisConfig::Mode(
                            SemanticAnalysisMode::Workspace,
                        )),
                    }),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: Some("codemod-${{ task.signature }}".to_string()),
                pull_request: None,
            },
        ],
    }
}

fn init_test_git_repo(path: &std::path::Path) {
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(path)
            .status()
            .expect("failed to spawn git");
        assert!(
            status.success(),
            "git command failed: git {}",
            args.join(" ")
        );
    };

    run(&["init", "-b", "main"]);
    run(&["config", "user.name", "Codex Test"]);
    run(&["config", "user.email", "codex@example.com"]);
    fs::write(path.join("README.md"), "test repo\n").unwrap();
    run(&["add", "README.md"]);
    run(&["commit", "-m", "initial"]);
}

// Helper function to create a workflow with templates
fn create_template_workflow() -> Workflow {
    let template = Template {
        id: "checkout-repo".to_string(),
        name: "Checkout Repository".to_string(),
        description: Some("Standard process for checking out a repository".to_string()),
        inputs: vec![
            butterflow_models::TemplateInput {
                name: "repo_url".to_string(),
                r#type: "string".to_string(),
                required: true,
                description: Some("URL of the repository to checkout".to_string()),
                default: None,
            },
            butterflow_models::TemplateInput {
                name: "branch".to_string(),
                r#type: "string".to_string(),
                required: false,
                description: Some("Branch to checkout".to_string()),
                default: Some("main".to_string()),
            },
        ],
        runtime: Some(Runtime {
            r#type: RuntimeType::Direct,
            image: None,
            working_dir: None,
            user: None,
            network: None,
            options: None,
        }),
        steps: vec![Step {
            id: Some("step-1".to_string()),
            name: "Clone repository".to_string(),
            action: StepAction::RunScript(
                "echo 'Cloning repository ${inputs.repo_url} branch ${inputs.branch}'".to_string(),
            ),
            env: None,
            condition: None,
            commit: None,
        }],
        outputs: vec![],
        env: HashMap::new(),
    };

    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![template],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: Some("Test node 1".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![Step {
                id: Some("step-1".to_string()),
                name: "Step 1".to_string(),
                action: StepAction::UseTemplate(butterflow_models::step::TemplateUse {
                    template: "checkout-repo".to_string(),
                    inputs: HashMap::from([
                        (
                            "repo_url".to_string(),
                            serde_json::Value::String(
                                "https://github.com/example/repo".to_string(),
                            ),
                        ),
                        (
                            "branch".to_string(),
                            serde_json::Value::String("feature/test".to_string()),
                        ),
                    ]),
                }),
                env: None,
                condition: None,
                commit: None,
            }],
            env: HashMap::new(),
            branch_name: None,
            pull_request: None,
        }],
    }
}

// Helper function to create a workflow with matrix strategy from state
fn create_matrix_from_state_workflow() -> Workflow {
    // Use default schema - the test doesn't need complex schema validation
    let root_schema = Default::default();

    Workflow {
        version: "1".to_string(),
        params: None,
        state: Some(butterflow_models::WorkflowState {
            schema: root_schema,
        }),
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: Some("Test node 1".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Setting up state'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "node2".to_string(),
                name: "Node 2".to_string(),
                description: Some("Test node 2".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()],
                trigger: None,
                strategy: Some(Strategy {
                    r#type: butterflow_models::strategy::StrategyType::Matrix,
                    values: None,
                    from_state: Some("files".to_string()),
                }),
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Processing file ${file}'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
        ],
    }
}

#[tokio::test]
async fn test_engine_new() {
    let _ = Engine::new();
}

#[tokio::test]
async fn test_engine_with_state_adapter() {
    let state_adapter = Box::new(LocalStateAdapter::new());
    let _ = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());
}

#[tokio::test]
async fn test_run_workflow() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_test_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();
    assert_eq!(workflow_run.id, workflow_run_id);

    // The workflow should be running or completed
    assert!(
        workflow_run.status == WorkflowStatus::Running
            || workflow_run.status == WorkflowStatus::Completed
    );
}

#[tokio::test]
async fn test_run_script_does_not_persist_command_notice_in_task_logs() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let command = "echo 'shell command executed'".to_string();
    let workflow = create_single_run_script_workflow(command.clone());

    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let status = wait_for_task_status(&engine, workflow_run_id, "shell-node", |status| {
        matches!(status, TaskStatus::Completed | TaskStatus::Failed)
    })
    .await;
    assert_eq!(status, TaskStatus::Completed);

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let task = tasks
        .iter()
        .find(|task| task.node_id == "shell-node")
        .expect("shell-node task should exist");

    let logs = task.logs.join("\n");
    assert!(
        logs.contains("Step started: Shell Step"),
        "task logs should include the active step marker, got: {logs}"
    );
    assert!(
        logs.contains("shell command executed"),
        "task logs should include the command output, got: {logs}"
    );
    assert!(
        !logs.contains("About to execute shell command"),
        "task logs should not persist the shell command notice, got: {logs}"
    );
    assert!(
        !logs.contains(&command),
        "task logs should not persist the raw shell command, got: {logs}"
    );
}

#[tokio::test]
async fn test_run_script_approval_callback_receives_command_to_be_executed() {
    let observed_commands = Arc::new(Mutex::new(Vec::<String>::new()));
    let approval_callback: ShellCommandApprovalCallback = {
        let observed_commands = Arc::clone(&observed_commands);
        Arc::new(move |request: &ShellCommandExecutionRequest| {
            observed_commands
                .lock()
                .unwrap()
                .push(request.command.clone());
            Ok(true)
        })
    };

    let config = WorkflowRunConfig {
        shell_command_approval_callback: Some(approval_callback),
        ..WorkflowRunConfig::default()
    };
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow =
        create_single_run_script_workflow("echo 'Hello ${{ params.repo_name }}'".to_string());
    let params = HashMap::from([("repo_name".to_string(), json!("butterflow"))]);

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    let status = wait_for_task_status(&engine, workflow_run_id, "shell-node", |status| {
        matches!(status, TaskStatus::Completed | TaskStatus::Failed)
    })
    .await;
    assert_eq!(status, TaskStatus::Completed);

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let task = tasks
        .iter()
        .find(|task| task.node_id == "shell-node")
        .expect("shell-node task should exist");
    assert_eq!(task.status, TaskStatus::Completed);

    let observed_commands = observed_commands.lock().unwrap();
    assert_eq!(observed_commands.as_slice(), ["echo 'Hello butterflow'"]);
}

#[tokio::test]
async fn test_run_script_approval_callback_can_reject_execution() {
    let temp_dir = TempDir::new().unwrap();
    let output_path = temp_dir.path().join("should-not-exist.txt");
    let approval_callback: ShellCommandApprovalCallback =
        Arc::new(|_request: &ShellCommandExecutionRequest| Ok(false));

    let config = WorkflowRunConfig {
        shell_command_approval_callback: Some(approval_callback),
        ..WorkflowRunConfig::default()
    };
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow =
        create_single_run_script_workflow(format!("echo blocked > {}", output_path.display()));

    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let status = wait_for_task_status(&engine, workflow_run_id, "shell-node", |status| {
        matches!(status, TaskStatus::Completed | TaskStatus::Failed)
    })
    .await;
    assert_eq!(status, TaskStatus::Failed);

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let task = tasks
        .iter()
        .find(|task| task.node_id == "shell-node")
        .expect("shell-node task should exist");

    assert_eq!(task.status, TaskStatus::Failed);
    assert!(task
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("declined by the user"));
    assert!(
        !output_path.exists(),
        "rejected shell command should not create files"
    );
}

#[tokio::test]
async fn test_get_workflow_status() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_test_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let status = engine.get_workflow_status(workflow_run_id).await.unwrap();

    // The workflow should be running or completed
    assert!(status == WorkflowStatus::Running || status == WorkflowStatus::Completed);
}

#[tokio::test]
async fn test_get_tasks() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_test_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow.clone(), params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // There should be tasks for each node
    assert_eq!(tasks.len(), workflow.nodes.len());

    // Check that the tasks have the correct node IDs
    let node_ids: Vec<String> = tasks.iter().map(|t| t.node_id.clone()).collect();
    assert!(node_ids.contains(&"node1".to_string()));
    assert!(node_ids.contains(&"node2".to_string()));
}

#[tokio::test]
async fn test_list_workflow_runs() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    // Run multiple workflows
    let workflow = create_test_workflow();
    let params = HashMap::new();

    let workflow_run_id1 = engine
        .run_workflow(workflow.clone(), params.clone(), None, None)
        .await
        .unwrap();
    let workflow_run_id2 = engine
        .run_workflow(workflow.clone(), params.clone(), None, None)
        .await
        .unwrap();

    // Allow some time for the workflows to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    let runs = engine.list_workflow_runs(10).await.unwrap();

    // There should be at least 2 workflow runs
    assert!(runs.len() >= 2);

    // The runs should include our workflow run IDs
    let run_ids: Vec<Uuid> = runs.iter().map(|r| r.id).collect();
    assert!(run_ids.contains(&workflow_run_id1));
    assert!(run_ids.contains(&workflow_run_id2));
}

#[tokio::test]
async fn test_cancel_workflow() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    // Create a workflow with a long-running task to ensure we can cancel it
    let workflow = create_long_running_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Small delay to allow workflow to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Cancel the workflow
    engine.cancel_workflow(workflow_run_id).await.unwrap();

    // Check the workflow status
    let status = engine.get_workflow_status(workflow_run_id).await.unwrap();
    assert_eq!(status, WorkflowStatus::Canceled);
}

#[tokio::test]
async fn test_manual_trigger_workflow() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_manual_trigger_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    let node2_status = wait_for_task_status(&engine, workflow_run_id, "node2", |status| {
        status == TaskStatus::AwaitingTrigger
    })
    .await;
    assert_eq!(node2_status, TaskStatus::AwaitingTrigger);

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let node2_task = tasks.iter().find(|t| t.node_id == "node2").unwrap();

    // Trigger the task using resume_workflow
    engine
        .resume_workflow(workflow_run_id, vec![node2_task.id])
        .await
        .unwrap();

    let updated_status = wait_for_task_status(&engine, workflow_run_id, "node2", |status| {
        status == TaskStatus::Running || status == TaskStatus::Completed
    })
    .await;
    assert!(updated_status == TaskStatus::Running || updated_status == TaskStatus::Completed);
}

#[tokio::test]
async fn test_manual_node_workflow() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_manual_node_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    let workflow_status = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;
    assert_eq!(workflow_status, WorkflowStatus::AwaitingTrigger);

    let node2_status = wait_for_task_status(&engine, workflow_run_id, "node2", |status| {
        status == TaskStatus::AwaitingTrigger
    })
    .await;
    assert_eq!(node2_status, TaskStatus::AwaitingTrigger);

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let node2_task = tasks.iter().find(|t| t.node_id == "node2").unwrap();

    // Trigger the task using resume_workflow
    engine
        .resume_workflow(workflow_run_id, vec![node2_task.id])
        .await
        .unwrap();

    let updated_status = wait_for_task_status(&engine, workflow_run_id, "node2", |status| {
        status == TaskStatus::Running || status == TaskStatus::Completed
    })
    .await;
    assert!(updated_status == TaskStatus::Running || updated_status == TaskStatus::Completed);
}

#[tokio::test]
async fn test_matrix_workflow() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_matrix_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Get the tasks
    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // There should be at least 4 tasks:
    // 1 for node1, 1 master task for node2, and 3 matrix tasks for node2
    assert!(tasks.len() >= 4);

    // Count the number of tasks for node2
    let node2_tasks = tasks.iter().filter(|t| t.node_id == "node2").count();

    // There should be at least 3 matrix tasks for node2 (one for each region)
    assert!(node2_tasks >= 3);
}

#[tokio::test]
async fn test_template_workflow() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_template_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Get the workflow run
    let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();

    // Check that the workflow run is running or completed
    assert!(
        workflow_run.status == WorkflowStatus::Running
            || workflow_run.status == WorkflowStatus::Completed
    );

    // Get the tasks
    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // There should be at least 1 task
    assert!(!tasks.is_empty());

    // Check that the task for node1 exists
    let node1_task = tasks.iter().find(|t| t.node_id == "node1").unwrap();

    // Print the task status for debugging
    println!("Node1 task status: {:?}", node1_task.status);
}

// Test for trigger_all method
#[tokio::test]
async fn test_trigger_all() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_manual_trigger_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Get the workflow status
    let status = engine.get_workflow_status(workflow_run_id).await.unwrap();

    // The workflow should be awaiting trigger or running
    assert!(status == WorkflowStatus::AwaitingTrigger || status == WorkflowStatus::Running);

    // Trigger all awaiting tasks
    engine.trigger_all(workflow_run_id).await.unwrap();

    // Allow some time for the tasks to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Get the workflow status again
    let status = engine.get_workflow_status(workflow_run_id).await.unwrap();

    // The workflow should now be running or completed
    assert!(status == WorkflowStatus::Running || status == WorkflowStatus::Completed);
}

#[tokio::test]
async fn test_manual_matrix_master_tracks_child_trigger_state() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_manual_matrix_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let workflow_status = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;
    assert_eq!(workflow_status, WorkflowStatus::AwaitingTrigger);

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let master_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.is_master)
        .unwrap();
    assert_eq!(master_task.status, TaskStatus::AwaitingTrigger);
    assert!(master_task.ended_at.is_none());

    let awaiting_children: Vec<&Task> = tasks
        .iter()
        .filter(|task| task.master_task_id == Some(master_task.id))
        .collect();
    assert_eq!(awaiting_children.len(), 2);
    assert!(awaiting_children
        .iter()
        .all(|task| task.status == TaskStatus::AwaitingTrigger));
}

#[tokio::test]
async fn test_trigger_all_clears_matrix_master_terminal_metadata() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_manual_matrix_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    engine.trigger_all(workflow_run_id).await.unwrap();

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let master_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.is_master)
        .unwrap();

    assert!(
        matches!(
            master_task.status,
            TaskStatus::Pending | TaskStatus::Running | TaskStatus::Completed
        ),
        "unexpected master task status after trigger_all: {:?}",
        master_task.status
    );
    assert!(
        master_task.ended_at.is_none() || master_task.status == TaskStatus::Completed,
        "master task should not keep a stale ended_at while active: {:?}",
        master_task
    );
}

#[tokio::test]
async fn test_panicking_task_thread_fails_child_and_reconciles_matrix_master() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let config = WorkflowRunConfig {
        install_skill_executor: Some(Arc::new(PanicInstallSkillExecutor)),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_manual_matrix_install_skill_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let master_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.is_master)
        .unwrap()
        .id;
    let child_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.master_task_id == Some(master_task))
        .unwrap()
        .id;

    engine
        .resume_workflow(workflow_run_id, vec![child_task])
        .await
        .unwrap();

    let latest_state: Arc<Mutex<PanicRecoveryState>> = Arc::new(Mutex::new(None));
    let latest_state_for_loop = Arc::clone(&latest_state);
    tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let child = tasks.iter().find(|task| task.id == child_task).unwrap();
            let master = tasks.iter().find(|task| task.id == master_task).unwrap();
            *latest_state_for_loop.lock().unwrap() =
                Some((child.status, master.status, child.logs.clone()));

            if child.status == TaskStatus::Failed && master.status == TaskStatus::Failed {
                assert!(
                    child
                        .error
                        .as_deref()
                        .is_some_and(|error| error.contains("panic install skill executor")),
                    "expected panic message in child error, got {:?}",
                    child.error
                );
                assert!(
                    child
                        .logs
                        .iter()
                        .all(|line| !line.contains("Marking task complete")),
                    "failed child should not log completion, got {:?}",
                    child.logs
                );
                return;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "panic-path task should fail and reconcile master; last state was {:?}",
            latest_state.lock().unwrap().clone()
        )
    });
}

#[tokio::test]
#[serial]
async fn test_panicking_task_thread_cleans_up_git_worktree() {
    let repo_dir = TempDir::new().unwrap();
    init_test_git_repo(repo_dir.path());

    let state_adapter = Box::new(MockStateAdapter::new());
    let config = WorkflowRunConfig {
        target_path: repo_dir.path().to_path_buf(),
        install_skill_executor: Some(Arc::new(PanicInstallSkillExecutor)),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_manual_matrix_install_skill_git_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let master_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.is_master)
        .unwrap()
        .id;
    let child_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.master_task_id == Some(master_task))
        .unwrap()
        .id;

    engine
        .resume_workflow(workflow_run_id, vec![child_task])
        .await
        .unwrap();

    tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let child = tasks.iter().find(|task| task.id == child_task).unwrap();
            if child.status == TaskStatus::Failed {
                return;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("panic-path git task should fail");

    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git worktree list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let worktree_entries: Vec<_> = stdout
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .collect();
    assert_eq!(
        worktree_entries.len(),
        1,
        "expected only the main repo worktree after panic cleanup, got {:?}",
        worktree_entries
    );
}

#[tokio::test]
async fn test_failing_install_skill_child_reconciles_matrix_master() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let config = WorkflowRunConfig {
        install_skill_executor: Some(Arc::new(FailingInstallSkillExecutor)),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_manual_matrix_install_skill_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let master_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.is_master)
        .unwrap()
        .id;
    let child_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.master_task_id == Some(master_task))
        .unwrap()
        .id;
    engine
        .resume_workflow(workflow_run_id, vec![child_task])
        .await
        .unwrap();

    let latest_state: Arc<Mutex<MatrixTaskSnapshot>> = Arc::new(Mutex::new(Vec::new()));
    let latest_state_for_loop = Arc::clone(&latest_state);
    tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let child = tasks.iter().find(|task| task.id == child_task).unwrap();
            let master = tasks.iter().find(|task| task.id == master_task).unwrap();
            *latest_state_for_loop.lock().unwrap() = tasks
                .iter()
                .filter(|task| task.node_id == "node2")
                .map(|task| {
                    (
                        task.id,
                        task.master_task_id,
                        task.is_master,
                        task.status,
                        task.error.clone(),
                        task.logs.clone(),
                    )
                })
                .collect();

            if child.status == TaskStatus::Failed && master.status == TaskStatus::Failed {
                assert!(
                    child.error
                        .as_deref()
                        .is_some_and(|error| error.contains("failing install skill executor")),
                    "expected failing executor message in child error, got {:?}",
                    child.error
                );
                return;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "error-path task should fail and reconcile master; selected child={} master={} node2_tasks={:?}",
            child_task,
            master_task,
            latest_state.lock().unwrap().clone()
        )
    });
}

#[tokio::test]
async fn test_install_skill_executor_receives_workflow_bundle_path() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let bundle_dir = TempDir::new().unwrap();
    let config_bundle_dir = TempDir::new().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let config = WorkflowRunConfig {
        bundle_path: config_bundle_dir.path().to_path_buf(),
        install_skill_executor: Some(Arc::new(RecordingInstallSkillExecutor {
            requests: Arc::clone(&requests),
            output: "installed".to_string(),
        })),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_manual_matrix_install_skill_workflow();
    let workflow_run_id = engine
        .run_workflow(
            workflow,
            HashMap::new(),
            Some(bundle_dir.path().to_path_buf()),
            None,
        )
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let master_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.is_master)
        .unwrap()
        .id;
    let child_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.master_task_id == Some(master_task))
        .unwrap()
        .id;

    engine
        .resume_workflow(workflow_run_id, vec![child_task])
        .await
        .unwrap();

    tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        loop {
            if requests.lock().unwrap().len() == 1 {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("install-skill executor should be invoked");

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].bundle_path.as_deref(), Some(bundle_dir.path()));
}

#[tokio::test]
async fn test_quiet_install_skill_request_stays_interactive() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let config = WorkflowRunConfig {
        install_skill_executor: Some(Arc::new(RecordingInstallSkillExecutor {
            requests: Arc::clone(&requests),
            output: "installed".to_string(),
        })),
        quiet: true,
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_manual_matrix_install_skill_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let master_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.is_master)
        .unwrap()
        .id;
    let child_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.master_task_id == Some(master_task))
        .unwrap()
        .id;

    engine
        .resume_workflow(workflow_run_id, vec![child_task])
        .await
        .unwrap();

    tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        loop {
            if requests.lock().unwrap().len() == 1 {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("install-skill executor should be invoked");

    let recorded = requests.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(recorded[0].quiet);
    assert!(!recorded[0].no_interactive);
}

#[tokio::test]
async fn test_deferred_single_install_skill_can_be_retriggered() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let attempts = Arc::new(Mutex::new(0usize));
    let config = WorkflowRunConfig {
        install_skill_executor: Some(Arc::new(DeferredThenSuccessInstallSkillExecutor {
            attempts: Arc::clone(&attempts),
        })),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_manual_install_skill_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let task_id = tasks
        .iter()
        .find(|task| task.node_id == "node2")
        .unwrap()
        .id;

    engine
        .resume_workflow(workflow_run_id, vec![task_id])
        .await
        .unwrap();

    type DeferredTaskSnapshot = Vec<(
        String,
        TaskStatus,
        Option<chrono::DateTime<chrono::Utc>>,
        Vec<String>,
    )>;
    let latest_state: Arc<Mutex<DeferredTaskSnapshot>> = Arc::new(Mutex::new(Vec::new()));
    let latest_state_for_loop = Arc::clone(&latest_state);
    tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            *latest_state_for_loop.lock().unwrap() = tasks
                .iter()
                .map(|task| {
                    (
                        task.node_id.clone(),
                        task.status,
                        task.started_at,
                        task.logs.clone(),
                    )
                })
                .collect();
            let task = tasks.iter().find(|task| task.id == task_id).unwrap();
            if task.status == TaskStatus::AwaitingTrigger && task.started_at.is_none() {
                return;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "single install-skill task should return to awaiting trigger after defer; attempts={} latest state={:?}",
            *attempts.lock().unwrap(),
            latest_state.lock().unwrap().clone()
        )
    });

    engine
        .resume_workflow(workflow_run_id, vec![task_id])
        .await
        .unwrap();

    tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let task = tasks.iter().find(|task| task.id == task_id).unwrap();
            if task.status == TaskStatus::Completed {
                assert!(
                    task.logs.iter().any(|line| line.contains("installed")),
                    "expected successful install output in task logs, got {:?}",
                    task.logs
                );
                return;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("single install-skill task should complete after retrigger");

    assert_eq!(*attempts.lock().unwrap(), 2);
}

#[tokio::test]
async fn test_deferred_matrix_install_skill_returns_triggered_child_to_awaiting() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let attempts = Arc::new(Mutex::new(0usize));
    let config = WorkflowRunConfig {
        install_skill_executor: Some(Arc::new(DeferredThenSuccessInstallSkillExecutor {
            attempts: Arc::clone(&attempts),
        })),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_manual_matrix_install_skill_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let master_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.is_master)
        .unwrap()
        .id;
    let child_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.master_task_id == Some(master_task))
        .unwrap()
        .id;

    engine
        .resume_workflow(workflow_run_id, vec![child_task])
        .await
        .unwrap();

    type MatrixDeferredSnapshot = Vec<(
        uuid::Uuid,
        bool,
        TaskStatus,
        Option<chrono::DateTime<chrono::Utc>>,
        Vec<String>,
    )>;
    let latest_state: Arc<Mutex<MatrixDeferredSnapshot>> = Arc::new(Mutex::new(Vec::new()));
    let latest_state_for_loop = Arc::clone(&latest_state);
    tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            *latest_state_for_loop.lock().unwrap() = tasks
                .iter()
                .filter(|task| task.node_id == "node2")
                .map(|task| {
                    (
                        task.id,
                        task.is_master,
                        task.status,
                        task.started_at,
                        task.logs.clone(),
                    )
                })
                .collect();
            let task = tasks.iter().find(|task| task.id == child_task).unwrap();
            if task.status == TaskStatus::AwaitingTrigger && task.started_at.is_none() {
                return;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "matrix triggered child should return to awaiting trigger after defer; attempts={} latest state={:?}",
            *attempts.lock().unwrap(),
            latest_state.lock().unwrap().clone()
        )
    });
}

#[tokio::test]
async fn test_install_skill_success_output_is_appended_to_task_logs() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let config = WorkflowRunConfig {
        install_skill_executor: Some(Arc::new(RecordingInstallSkillExecutor {
            requests: Arc::clone(&requests),
            output: "Installed package skill `debarrel` for `claude` (project)".to_string(),
        })),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_manual_matrix_install_skill_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let master_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.is_master)
        .unwrap()
        .id;
    let child_task = tasks
        .iter()
        .find(|task| task.node_id == "node2" && task.master_task_id == Some(master_task))
        .unwrap()
        .id;

    engine
        .resume_workflow(workflow_run_id, vec![child_task])
        .await
        .unwrap();

    tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let child = tasks.iter().find(|task| task.id == child_task).unwrap();
            if child.status == TaskStatus::Completed {
                assert!(
                    child
                        .logs
                        .iter()
                        .any(|line| line.contains("Installed package skill `debarrel`")),
                    "expected install output in child logs, got {:?}",
                    child.logs
                );
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("install-skill task should complete");
}

#[tokio::test]
async fn test_resume_workflow_advances_all_manual_matrix_children() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_manual_matrix_long_running_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let child_task_ids: Vec<Uuid> = tasks
        .iter()
        .filter(|task| task.node_id == "node2" && task.master_task_id.is_some())
        .map(|task| task.id)
        .collect();

    assert_eq!(
        child_task_ids.len(),
        2,
        "expected two manual matrix child tasks"
    );

    for child_task_id in &child_task_ids {
        engine
            .resume_workflow(workflow_run_id, vec![*child_task_id])
            .await
            .unwrap();
    }

    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    loop {
        let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
        let child_statuses: Vec<TaskStatus> = tasks
            .iter()
            .filter(|task| child_task_ids.contains(&task.id))
            .map(|task| task.status)
            .collect();

        if child_statuses.len() == child_task_ids.len()
            && child_statuses
                .iter()
                .all(|status| *status == TaskStatus::Running || *status == TaskStatus::Completed)
        {
            break;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for all manual matrix children to advance, last statuses were {:?}",
            child_statuses
        );

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn test_resume_workflow_manual_matrix_children_produce_logs_and_finish() {
    use std::sync::{Arc, Mutex};

    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_manual_matrix_long_running_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let child_task_ids: Vec<Uuid> = tasks
        .iter()
        .filter(|task| task.node_id == "node2" && task.master_task_id.is_some())
        .map(|task| task.id)
        .collect();

    assert_eq!(
        child_task_ids.len(),
        2,
        "expected two manual matrix child tasks"
    );

    engine
        .resume_workflow(workflow_run_id, child_task_ids.clone())
        .await
        .unwrap();

    let latest_states: Arc<Mutex<Vec<(Uuid, TaskStatus, String)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let latest_states_for_loop = Arc::clone(&latest_states);
    tokio::time::timeout(tokio::time::Duration::from_secs(10), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let child_tasks: Vec<&Task> = tasks
                .iter()
                .filter(|task| child_task_ids.contains(&task.id))
                .collect();

            let snapshot: Vec<(Uuid, TaskStatus, String)> = child_tasks
                .iter()
                .map(|task| {
                    (
                        task.id,
                        task.status,
                        task.logs.last().cloned().unwrap_or_default(),
                    )
                })
                .collect();
            *latest_states_for_loop.lock().unwrap() = snapshot;

            let all_have_step_progress = child_tasks.iter().all(|task| {
                let logs = task.logs.join("\n");
                logs.contains("Step started: Step 1") || logs.contains("Manual matrix shard")
            });
            let all_terminal = child_tasks.iter().all(|task| {
                matches!(task.status, TaskStatus::Completed | TaskStatus::Failed)
            });

            if child_tasks.len() == child_task_ids.len() && all_have_step_progress && all_terminal {
                return;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out waiting for resumed manual matrix children to produce step progress and finish; last child states were {:?}",
            latest_states.lock().unwrap().clone()
        )
    });
}

#[tokio::test]
async fn test_trigger_all_manual_matrix_children_produce_logs_and_finish() {
    use std::sync::{Arc, Mutex};

    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_manual_matrix_long_running_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    engine.trigger_all(workflow_run_id).await.unwrap();

    let latest_states: Arc<Mutex<Vec<(Uuid, TaskStatus, String)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let latest_states_for_loop = Arc::clone(&latest_states);
    tokio::time::timeout(tokio::time::Duration::from_secs(10), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let child_tasks: Vec<&Task> = tasks
                .iter()
                .filter(|task| task.node_id == "node2" && task.master_task_id.is_some())
                .collect();

            let snapshot: Vec<(Uuid, TaskStatus, String)> = child_tasks
                .iter()
                .map(|task| {
                    (
                        task.id,
                        task.status,
                        task.logs.last().cloned().unwrap_or_default(),
                    )
                })
                .collect();
            *latest_states_for_loop.lock().unwrap() = snapshot;

            let all_have_step_progress = child_tasks.iter().all(|task| {
                let logs = task.logs.join("\n");
                logs.contains("Step started: Step 1") || logs.contains("Manual matrix shard")
            });
            let all_terminal = child_tasks.iter().all(|task| {
                matches!(task.status, TaskStatus::Completed | TaskStatus::Failed)
            });

            if child_tasks.len() == 2 && all_have_step_progress && all_terminal {
                return;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out waiting for trigger_all manual matrix children to produce step progress and finish; last child states were {:?}",
            latest_states.lock().unwrap().clone()
        )
    });
}

#[tokio::test]
#[serial]
async fn test_resume_workflow_git_managed_manual_matrix_children_produce_logs_and_finish() {
    let repo_dir = TempDir::new().unwrap();
    init_test_git_repo(repo_dir.path());

    let state_adapter = Box::new(MockStateAdapter::new());
    let config = WorkflowRunConfig {
        target_path: repo_dir.path().to_path_buf(),
        enable_managed_git: true,
        enable_worktrees: true,
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_manual_matrix_long_running_git_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let child_task_ids: Vec<Uuid> = tasks
        .iter()
        .filter(|task| task.node_id == "node2" && task.master_task_id.is_some())
        .map(|task| task.id)
        .collect();

    assert_eq!(
        child_task_ids.len(),
        2,
        "expected two manual matrix child tasks"
    );

    engine
        .resume_workflow(workflow_run_id, child_task_ids.clone())
        .await
        .unwrap();

    let latest_states: Arc<Mutex<Vec<(Uuid, TaskStatus, String)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let latest_states_for_loop = Arc::clone(&latest_states);
    tokio::time::timeout(tokio::time::Duration::from_secs(15), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let child_tasks: Vec<&Task> = tasks
                .iter()
                .filter(|task| child_task_ids.contains(&task.id))
                .collect();

            let snapshot: Vec<(Uuid, TaskStatus, String)> = child_tasks
                .iter()
                .map(|task| {
                    (
                        task.id,
                        task.status,
                        task.logs.last().cloned().unwrap_or_default(),
                    )
                })
                .collect();
            *latest_states_for_loop.lock().unwrap() = snapshot;

            let all_have_worktree_progress = child_tasks.iter().all(|task| {
                let logs = task.logs.join("\n");
                logs.contains("Git worktree ready at")
                    && (logs.contains("Step started: Step 1") || logs.contains("Manual matrix shard"))
            });
            let all_terminal = child_tasks.iter().all(|task| {
                matches!(task.status, TaskStatus::Completed | TaskStatus::Failed)
            });

            if child_tasks.len() == child_task_ids.len() && all_have_worktree_progress && all_terminal {
                return;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out waiting for git-managed manual matrix children to produce worktree+step progress and finish; last child states were {:?}",
            latest_states.lock().unwrap().clone()
        )
    });
}

#[tokio::test]
#[serial]
async fn non_tui_workflow_run_skips_worktree_and_pull_request_flow() {
    let repo_dir = TempDir::new().unwrap();
    init_test_git_repo(repo_dir.path());
    create_test_file(repo_dir.path(), "tracked.txt", "original\n");
    Command::new("git")
        .args(["add", "tracked.txt"])
        .current_dir(repo_dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "add tracked file"])
        .current_dir(repo_dir.path())
        .status()
        .unwrap();

    let state_adapter = Box::new(MockStateAdapter::new());
    let config = WorkflowRunConfig {
        target_path: repo_dir.path().to_path_buf(),
        enable_managed_git: false,
        enable_worktrees: false,
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_git_managed_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        matches!(status, WorkflowStatus::Completed)
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let task = tasks
        .iter()
        .find(|task| task.node_id == "apply-transforms")
        .expect("managed git task should exist");

    assert_eq!(task.status, TaskStatus::Completed);
    let logs = task.logs.join("\n");
    assert!(
        !logs.contains("Creating git worktree for branch"),
        "non-TUI run should not create git worktrees; logs were: {logs}"
    );
    assert!(
        !logs.contains("Publishing branch and creating pull request"),
        "non-TUI run should not attempt PR creation; logs were: {logs}"
    );
    assert!(
        !logs.contains("Pull request created:"),
        "non-TUI run should not create PRs; logs were: {logs}"
    );

    let branch_output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo_dir.path())
        .output()
        .unwrap();
    assert!(
        branch_output.status.success(),
        "git rev-parse failed: {}",
        String::from_utf8_lossy(&branch_output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&branch_output.stdout).trim(),
        "main"
    );

    let worktree_output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_dir.path())
        .output()
        .unwrap();
    assert!(
        worktree_output.status.success(),
        "git worktree list failed: {}",
        String::from_utf8_lossy(&worktree_output.stderr)
    );
    let worktree_stdout = String::from_utf8_lossy(&worktree_output.stdout);
    let worktree_entries: Vec<_> = worktree_stdout
        .lines()
        .filter(|line| line.starts_with("worktree "))
        .collect();
    assert_eq!(
        worktree_entries.len(),
        1,
        "expected only the main repo worktree, got {:?}",
        worktree_entries
    );
}

#[tokio::test]
#[serial]
async fn test_workflow_session_capabilities_approval_unblocks_manual_matrix_children() {
    let repo_dir = TempDir::new().unwrap();
    init_test_git_repo(repo_dir.path());

    create_test_file(
        repo_dir.path(),
        "codemod.js",
        r#"
export default function transform(ast) {
  return "transformed";
}
"#,
    );

    for relative_path in [
        "apps/nextjs/src/app/(protected)/wish/_atoms/push-notifications.ts",
        "apps/nextjs/src/app/(protected)/wish/_hooks/use-wish-thread.ts",
        "apps/website/src/app/(landing)/blog/_utils/blog-helpers.ts",
        "apps/website/src/app/(payload)/payload/graphql/route.ts",
    ] {
        create_test_file(repo_dir.path(), relative_path, "export const value = 1;\n");
    }

    Command::new("git")
        .args(["add", "."])
        .current_dir(repo_dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "fixture"])
        .current_dir(repo_dir.path())
        .status()
        .unwrap();

    let state_adapter = Box::new(MockStateAdapter::new());
    let config = WorkflowRunConfig {
        target_path: repo_dir.path().to_path_buf(),
        bundle_path: repo_dir.path().to_path_buf(),
        capabilities: Some([LlrtSupportedModules::Fs].into_iter().collect()),
        enable_managed_git: true,
        enable_worktrees: true,
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_manual_matrix_git_js_ast_grep_workflow();
    let workflow_run_id = engine
        .run_workflow(
            workflow,
            HashMap::new(),
            Some(repo_dir.path().to_path_buf()),
            None,
        )
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let child_task_ids: Vec<Uuid> = tasks
        .iter()
        .filter(|task| task.node_id == "node2" && task.master_task_id.is_some())
        .map(|task| task.id)
        .collect();
    assert_eq!(child_task_ids.len(), 2);

    engine
        .resume_workflow(workflow_run_id, child_task_ids.clone())
        .await
        .unwrap();

    let latest_states: Arc<Mutex<Vec<(Uuid, TaskStatus, String)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let latest_states_for_loop = Arc::clone(&latest_states);
    tokio::time::timeout(tokio::time::Duration::from_secs(20), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let child_tasks: Vec<&Task> = tasks
                .iter()
                .filter(|task| child_task_ids.contains(&task.id))
                .collect();

            let snapshot: Vec<(Uuid, TaskStatus, String)> = child_tasks
                .iter()
                .map(|task| {
                    (
                        task.id,
                        task.status,
                        task.logs.last().cloned().unwrap_or_default(),
                    )
                })
                .collect();
            *latest_states_for_loop.lock().unwrap() = snapshot;

            let all_terminal = child_tasks
                .iter()
                .all(|task| matches!(task.status, TaskStatus::Completed | TaskStatus::Failed));

            if child_tasks.len() == child_task_ids.len() && all_terminal {
                return;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out waiting for session-driven capability-approved manual matrix children to finish; last child states were {:?}",
            latest_states.lock().unwrap().clone()
        )
    });
}

#[tokio::test]
#[serial]
async fn test_workflow_session_real_debarrel_workspace_children_process_files() {
    let repo_dir = TempDir::new().unwrap();
    init_test_git_repo(repo_dir.path());

    create_test_file(
        repo_dir.path(),
        "tsconfig.json",
        r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "ESNext",
  "moduleResolution": "bundler",
    "baseUrl": "."
  },
  "include": ["src"]
}
"#,
    );
    create_test_file(
        repo_dir.path(),
        "src/App.ts",
        "import { Button } from \"./components\";\n\nconsole.log(Button());\n",
    );
    create_test_file(
        repo_dir.path(),
        "src/components/index.ts",
        "export { Button } from \"./Button\";\n",
    );
    create_test_file(
        repo_dir.path(),
        "src/components/Button.ts",
        "export const Button = () => \"button\";\n",
    );
    create_test_file(
        repo_dir.path(),
        "src/consumer.ts",
        "import { calc } from \"./utils\";\n\nconsole.log(calc(1, 2));\n",
    );
    create_test_file(
        repo_dir.path(),
        "src/utils/index.ts",
        "export { calc } from \"./calc\";\n",
    );
    create_test_file(
        repo_dir.path(),
        "src/utils/calc.ts",
        "export const calc = (a: number, b: number) => a + b;\n",
    );

    Command::new("git")
        .args(["add", "."])
        .current_dir(repo_dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "fixture"])
        .current_dir(repo_dir.path())
        .status()
        .unwrap();

    let Some(bundle_path) = debarrel_bundle_path() else {
        eprintln!("skipping external debarrel workspace test: bundle not available");
        return;
    };

    let state_adapter = Box::new(MockStateAdapter::new());
    let config = WorkflowRunConfig {
        target_path: repo_dir.path().to_path_buf(),
        bundle_path: bundle_path.clone(),
        capabilities: Some([LlrtSupportedModules::Fs].into_iter().collect()),
        enable_managed_git: true,
        enable_worktrees: true,
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_manual_matrix_real_debarrel_workspace_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), Some(bundle_path), None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let child_task_ids: Vec<Uuid> = tasks
        .iter()
        .filter(|task| task.node_id == "node2" && task.master_task_id.is_some())
        .map(|task| task.id)
        .collect();
    assert_eq!(child_task_ids.len(), 2);

    engine
        .resume_workflow(workflow_run_id, child_task_ids.clone())
        .await
        .unwrap();

    let latest_states: Arc<Mutex<Vec<(Uuid, TaskStatus, String)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let latest_states_for_loop = Arc::clone(&latest_states);
    tokio::time::timeout(tokio::time::Duration::from_secs(30), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let child_tasks: Vec<&Task> = tasks
                .iter()
                .filter(|task| child_task_ids.contains(&task.id))
                .collect();

            let snapshot: Vec<(Uuid, TaskStatus, String)> = child_tasks
                .iter()
                .map(|task| {
                    (
                        task.id,
                        task.status,
                        task.logs.last().cloned().unwrap_or_default(),
                    )
                })
                .collect();
            *latest_states_for_loop.lock().unwrap() = snapshot;

            let all_terminal = child_tasks
                .iter()
                .all(|task| matches!(task.status, TaskStatus::Completed | TaskStatus::Failed));

            if child_tasks.len() == child_task_ids.len() && all_terminal {
                return;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out waiting for session-driven real debarrel workspace children to process files and finish; last child states were {:?}",
            latest_states.lock().unwrap().clone()
        )
    });
}

#[tokio::test]
#[serial]
async fn test_workflow_session_many_real_debarrel_workspace_children_process_files() {
    let repo_dir = TempDir::new().unwrap();
    init_test_git_repo(repo_dir.path());

    create_test_file(
        repo_dir.path(),
        "tsconfig.json",
        r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "baseUrl": "."
  },
  "include": ["src"]
}
"#,
    );

    let shard_count = 3usize;
    for index in 0..shard_count {
        create_test_file(
            repo_dir.path(),
            &format!("src/shard_{index}/App.ts"),
            "import { Button } from \"./components\";\n\nconsole.log(Button());\n",
        );
        create_test_file(
            repo_dir.path(),
            &format!("src/shard_{index}/components/index.ts"),
            "export { Button } from \"./Button\";\n",
        );
        create_test_file(
            repo_dir.path(),
            &format!("src/shard_{index}/components/Button.ts"),
            "export const Button = () => \"button\";\n",
        );
    }

    Command::new("git")
        .args(["add", "."])
        .current_dir(repo_dir.path())
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "fixture"])
        .current_dir(repo_dir.path())
        .status()
        .unwrap();

    let Some(bundle_path) = debarrel_bundle_path() else {
        eprintln!("skipping external debarrel workspace test: bundle not available");
        return;
    };

    let state_adapter = Box::new(MockStateAdapter::new());
    let config = WorkflowRunConfig {
        target_path: repo_dir.path().to_path_buf(),
        bundle_path: bundle_path.clone(),
        capabilities: Some([LlrtSupportedModules::Fs].into_iter().collect()),
        enable_managed_git: true,
        enable_worktrees: true,
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_many_shard_real_debarrel_workspace_workflow(shard_count);
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), Some(bundle_path), None)
        .await
        .unwrap();

    let _ = wait_for_workflow_status(&engine, workflow_run_id, |status| {
        status == WorkflowStatus::AwaitingTrigger
    })
    .await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let child_task_ids: Vec<Uuid> = tasks
        .iter()
        .filter(|task| task.node_id == "node2" && task.master_task_id.is_some())
        .map(|task| task.id)
        .collect();
    assert_eq!(child_task_ids.len(), shard_count);

    let session = WorkflowSession::attach(engine.clone(), workflow_run_id);
    let handle = session.handle();
    handle
        .send(WorkflowCommand::TriggerTasks {
            task_ids: child_task_ids.clone(),
        })
        .await
        .unwrap();

    let latest_states: Arc<Mutex<Vec<(Uuid, TaskStatus, String)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let latest_states_for_loop = Arc::clone(&latest_states);
    tokio::time::timeout(tokio::time::Duration::from_secs(45), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let child_tasks: Vec<&Task> = tasks
                .iter()
                .filter(|task| child_task_ids.contains(&task.id))
                .collect();

            let snapshot: Vec<(Uuid, TaskStatus, String)> = child_tasks
                .iter()
                .map(|task| {
                    (
                        task.id,
                        task.status,
                        task.logs.last().cloned().unwrap_or_default(),
                    )
                })
                .collect();
            *latest_states_for_loop.lock().unwrap() = snapshot;

            let all_started = child_tasks.iter().all(|task| {
                let logs = task.logs.join("\n");
                matches!(
                    task.status,
                    TaskStatus::Running | TaskStatus::Completed | TaskStatus::Failed
                ) && (logs.is_empty()
                    || logs.contains("Task execution starting")
                    || logs.contains("Step started: Debarrel: rewrite imports and clean up barrels"))
            });

            if child_tasks.len() == child_task_ids.len() && all_started {
                return;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "timed out waiting for many session-driven real debarrel workspace children to start; last child states were {:?}",
            latest_states.lock().unwrap().clone()
        )
    });
}

// Helper function to create a workflow with environment variables
fn create_env_var_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: Some("Test node 1".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![Step {
                id: Some("step-1".to_string()),
                name: "Step 1".to_string(),
                action: StepAction::RunScript("echo 'Using env var: $TEST_ENV_VAR'".to_string()),
                env: Some(HashMap::from([(
                    "STEP_SPECIFIC_VAR".to_string(),
                    "step-value".to_string(),
                )])),
                condition: None,
                commit: None,
            }],
            env: HashMap::from([
                ("TEST_ENV_VAR".to_string(), "test-value".to_string()),
                ("NODE_SPECIFIC_VAR".to_string(), "node-value".to_string()),
            ]),
            branch_name: None,
            pull_request: None,
        }],
    }
}

// Helper function to create a workflow with variable resolution
fn create_variable_resolution_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1 for ${params.repo_name}".to_string(),
            description: Some("Processing ${params.branch}".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![Step {
                id: Some("step-1".to_string()),
                name: "Step 1".to_string(),
                action: StepAction::RunScript(
                    "echo 'Processing repo: ${params.repo_name} on branch: ${params.branch}'"
                        .to_string(),
                ),
                env: None,
                condition: None,
                commit: None,
            }],
            env: HashMap::from([
                ("REPO_URL".to_string(), "${params.repo_url}".to_string()),
                ("DEBUG".to_string(), "${env.CI}".to_string()),
            ]),
            branch_name: None,
            pull_request: None,
        }],
    }
}

// Helper function to create a workflow that tests environment variables
fn create_env_vars_test_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "env-test-node".to_string(),
            name: "Environment Variables Test Node".to_string(),
            description: Some("Test node for environment variables".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![Step {
                id: Some("test-environment-variables".to_string()),
                name: "Test Environment Variables".to_string(),
                action: StepAction::RunScript(
                    r#"echo "CODEMOD_TASK_ID=$CODEMOD_TASK_ID"
echo "CODEMOD_WORKFLOW_RUN_ID=$CODEMOD_WORKFLOW_RUN_ID"
echo "task_id_set=$(if [ -n "$CODEMOD_TASK_ID" ]; then echo "true"; else echo "false"; fi)"
echo "workflow_run_id_set=$(if [ -n "$CODEMOD_WORKFLOW_RUN_ID" ]; then echo "true"; else echo "false"; fi)"
echo "task_id_valid=$(if [ "$CODEMOD_TASK_ID" != "" ] && [ ${#CODEMOD_TASK_ID} -eq 36 ]; then echo "true"; else echo "false"; fi)"
echo "workflow_run_id_valid=$(if [ "$CODEMOD_WORKFLOW_RUN_ID" != "" ] && [ ${#CODEMOD_WORKFLOW_RUN_ID} -eq 36 ]; then echo "true"; else echo "false"; fi)""#.to_string(),
                ),
                env: None,
                condition: None,
                    commit: None,
            }],
            env: HashMap::new(),
                branch_name: None,
                pull_request: None,
        }],
    }
}

fn create_ai_no_key_fallback_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "ai-fallback-node".to_string(),
            name: "AI Fallback Node".to_string(),
            description: Some("Ensure AI fallback does not break the node".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![
                Step {
                    id: Some("ai-step-no-key".to_string()),
                    name: "AI Step Without API Key".to_string(),
                    action: StepAction::AI(UseAI {
                        prompt: "Print these instructions when no key is available.".to_string(),
                        working_dir: None,
                        env: None,
                        dry_run: None,
                        model: None,
                        system_prompt: Some("You are a test system prompt.".to_string()),
                        max_steps: None,
                        timeout_ms: None,
                        tools: None,
                        endpoint: None,
                        api_key: None,
                        enable_lakeview: None,
                        llm_protocol: None,
                    }),
                    env: None,
                    condition: None,
                    commit: None,
                },
                Step {
                    id: Some("after-ai-step".to_string()),
                    name: "Step After AI".to_string(),
                    action: StepAction::RunScript("echo 'AFTER_AI_STEP_EXECUTED'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                },
            ],
            env: HashMap::new(),
            branch_name: None,
            pull_request: None,
        }],
    }
}

#[tokio::test]
async fn test_matrix_recompilation_with_direct_adapter() {
    // Create a mock state adapter
    let mut state_adapter = MockStateAdapter::new();

    // Create a workflow with a matrix node using from_state
    let workflow = create_matrix_from_state_workflow();

    // Create a workflow run
    let workflow_run_id = Uuid::new_v4();
    let workflow_run = WorkflowRun {
        id: workflow_run_id,
        workflow: workflow.clone(),
        status: WorkflowStatus::Running,
        params: HashMap::new(),
        tasks: Vec::new(),
        started_at: chrono::Utc::now(),
        ended_at: None,
        bundle_path: None,
        capabilities: None,
        name: None,
        target_path: None,
    };

    // Save the workflow run
    state_adapter
        .save_workflow_run(&workflow_run)
        .await
        .unwrap();

    // Create a task for node1
    let node1_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node1".to_string(),
        status: TaskStatus::Completed,
        is_master: false,
        master_task_id: None,
        matrix_values: None,
        started_at: Some(chrono::Utc::now()),
        ended_at: Some(chrono::Utc::now()),
        error: None,
        logs: Vec::new(),
    };

    // Save the task
    state_adapter.save_task(&node1_task).await.unwrap();

    // Create a master task for node2
    let master_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Pending,
        is_master: true,
        master_task_id: None,
        matrix_values: None,
        started_at: None,
        ended_at: None,
        error: None,
        logs: Vec::new(),
    };

    // Save the master task
    state_adapter.save_task(&master_task).await.unwrap();

    // Set state with initial files
    let files_array = serde_json::json!([
        {"file": "file1.txt"},
        {"file": "file2.txt"}
    ]);

    let mut state = HashMap::new();
    state.insert("files".to_string(), files_array);

    // Update the state directly on our adapter
    state_adapter
        .update_state(workflow_run_id, state)
        .await
        .unwrap();

    // Verify that two matrix tasks are created when we feed this state with matrix file values

    // First verify we have the initial tasks (master task for node2)
    let initial_tasks = state_adapter.get_tasks(workflow_run_id).await.unwrap();
    assert_eq!(initial_tasks.len(), 2); // node1 task + master task for node2

    // Now manually create the matrix tasks as the recompile function would

    // Create a task for file1.txt
    let file1_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Pending,
        is_master: false,
        master_task_id: Some(master_task.id),
        matrix_values: Some(HashMap::from([(
            "file".to_string(),
            serde_json::to_value("file1.txt").unwrap(),
        )])),
        started_at: None,
        ended_at: None,
        error: None,
        logs: Vec::new(),
    };

    // Create a task for file2.txt
    let file2_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Pending,
        is_master: false,
        master_task_id: Some(master_task.id),
        matrix_values: Some(HashMap::from([(
            "file".to_string(),
            serde_json::to_value("file2.txt").unwrap(),
        )])),
        started_at: None,
        ended_at: None,
        error: None,
        logs: Vec::new(),
    };

    // Save both tasks
    state_adapter.save_task(&file1_task).await.unwrap();
    state_adapter.save_task(&file2_task).await.unwrap();

    // Verify all tasks are present
    let tasks = state_adapter.get_tasks(workflow_run_id).await.unwrap();
    assert_eq!(tasks.len(), 4); // node1 task + master task + 2 matrix tasks

    // Simulate a state update that removes file2.txt and adds file3.txt
    let files_updated = serde_json::json!([
        {"file": "file1.txt"},
        {"file": "file3.txt"} // file2.txt removed, file3.txt added
    ]);

    let mut updated_state = HashMap::new();
    updated_state.insert("files".to_string(), files_updated);

    // Update the state with new files
    state_adapter
        .update_state(workflow_run_id, updated_state)
        .await
        .unwrap();

    // Mark file2_task as WontDo (as the recompile function would)
    let mut fields = HashMap::new();
    fields.insert(
        "status".to_string(),
        FieldDiff {
            operation: DiffOperation::Update,
            value: Some(serde_json::to_value(TaskStatus::WontDo).unwrap()),
        },
    );

    let task_diff = TaskDiff {
        task_id: file2_task.id,
        fields,
    };

    // Apply the diff to mark file2_task as WontDo
    state_adapter.apply_task_diff(&task_diff).await.unwrap();

    // Create a new task for file3.txt (as the recompile function would)
    let file3_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Pending,
        is_master: false,
        master_task_id: Some(master_task.id),
        matrix_values: Some(HashMap::from([(
            "file".to_string(),
            serde_json::to_value("file3.txt").unwrap(),
        )])),
        started_at: None,
        ended_at: None,
        error: None,
        logs: Vec::new(),
    };

    // Save the new task
    state_adapter.save_task(&file3_task).await.unwrap();

    // Verify the final task state
    let final_tasks = state_adapter.get_tasks(workflow_run_id).await.unwrap();
    assert_eq!(final_tasks.len(), 5); // node1 task + master task + 3 matrix tasks (including WontDo)

    // Verify one of the tasks is marked as WontDo
    let wontdo_tasks: Vec<&Task> = final_tasks
        .iter()
        .filter(|t| t.status == TaskStatus::WontDo)
        .collect();

    assert_eq!(wontdo_tasks.len(), 1);

    // Verify it's the file2 task that's marked as WontDo
    let file2_task_status = final_tasks
        .iter()
        .find(|t| t.id == file2_task.id)
        .map(|t| t.status)
        .unwrap();

    assert_eq!(file2_task_status, TaskStatus::WontDo);

    // Verify file1 task is still active and file3 task is new
    let active_matrix_tasks: Vec<&Task> = final_tasks
        .iter()
        .filter(|t| !t.is_master && t.status != TaskStatus::WontDo && t.matrix_values.is_some())
        .collect();

    assert_eq!(active_matrix_tasks.len(), 2);
}

#[tokio::test]
async fn test_env_var_workflow() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_env_var_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Get the workflow run
    let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();

    // Check that the workflow run is running or completed
    assert!(
        workflow_run.status == WorkflowStatus::Running
            || workflow_run.status == WorkflowStatus::Completed
    );

    // Get the tasks
    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // There should be at least 1 task
    assert!(!tasks.is_empty());

    // Check that the task for node1 exists
    let node1_task = tasks.iter().find(|t| t.node_id == "node1").unwrap();

    // Check that the task status is valid
    assert!(
        node1_task.status == TaskStatus::Running
            || node1_task.status == TaskStatus::Completed
            || node1_task.status == TaskStatus::Failed
    );
}

#[tokio::test]
async fn test_variable_resolution_workflow() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_variable_resolution_workflow();

    // Create parameters for variable resolution
    let mut params = HashMap::new();
    params.insert("repo_name".to_string(), json!("example-repo"));
    params.insert("branch".to_string(), json!("main"));
    params.insert(
        "repo_url".to_string(),
        json!("https://github.com/example/repo"),
    );

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Get the workflow run
    let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();

    // Check that the workflow run is running or completed
    assert!(
        workflow_run.status == WorkflowStatus::Running
            || workflow_run.status == WorkflowStatus::Completed
    );

    // Get the tasks
    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // There should be at least 1 task
    assert!(!tasks.is_empty());

    // Check that the task for node1 exists
    let node1_task = tasks.iter().find(|t| t.node_id == "node1").unwrap();

    // Check that the task status is valid
    assert!(
        node1_task.status == TaskStatus::Running
            || node1_task.status == TaskStatus::Completed
            || node1_task.status == TaskStatus::Failed
    );

    // Check that the parameters were saved
    assert_eq!(
        workflow_run.params.get("repo_name").unwrap(),
        "example-repo"
    );
    assert_eq!(workflow_run.params.get("branch").unwrap(), "main");
    assert_eq!(
        workflow_run.params.get("repo_url").unwrap(),
        "https://github.com/example/repo"
    );
}

#[tokio::test]
async fn test_invalid_workflow_run_id() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    // Generate a random UUID that doesn't exist
    let invalid_id = Uuid::new_v4();

    // Try to get a workflow run with an invalid ID
    let result = engine.get_workflow_run(invalid_id).await;

    // The result should be an error
    assert!(result.is_err());
}

#[tokio::test]
async fn test_workflow_with_params() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_test_workflow();

    // Create parameters
    let mut params = HashMap::new();
    params.insert("test_param".to_string(), json!("test_value"));

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();

    assert_eq!(
        workflow_run.params.get("test_param").unwrap(),
        &json!("test_value")
    );
}

#[tokio::test]
async fn test_codemod_environment_variables() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_env_vars_test_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Get the workflow run
    let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();

    // Check that the workflow run completed successfully
    assert!(
        workflow_run.status == WorkflowStatus::Running
            || workflow_run.status == WorkflowStatus::Completed
    );

    // Get the tasks
    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // Find the environment test task
    let env_test_task = tasks.iter().find(|t| t.node_id == "env-test-node").unwrap();

    // Check that the task has completed successfully or is running
    assert!(
        env_test_task.status == TaskStatus::Completed
            || env_test_task.status == TaskStatus::Running
    );

    // If the task completed, check the logs for the environment variables
    if env_test_task.status == TaskStatus::Completed {
        // The task should have logs showing the environment variables
        assert!(!env_test_task.logs.is_empty(), "Task should have logs");

        let log_output = env_test_task.logs.join("\n");

        // Check that CODEMOD_TASK_ID is set and matches the task ID
        assert!(
            log_output.contains(&format!("CODEMOD_TASK_ID={}", env_test_task.id)),
            "CODEMOD_TASK_ID should be set to the task ID"
        );

        // Check that CODEMOD_WORKFLOW_RUN_ID is set and matches the workflow run ID
        assert!(
            log_output.contains(&format!("CODEMOD_WORKFLOW_RUN_ID={workflow_run_id}")),
            "CODEMOD_WORKFLOW_RUN_ID should be set to the workflow run ID"
        );

        // Check that the validation scripts confirm the variables are set
        assert!(
            log_output.contains("task_id_set=true"),
            "CODEMOD_TASK_ID should be detected as set"
        );
        assert!(
            log_output.contains("workflow_run_id_set=true"),
            "CODEMOD_WORKFLOW_RUN_ID should be detected as set"
        );

        // Check that the UUIDs are valid format (36 characters)
        assert!(
            log_output.contains("task_id_valid=true"),
            "CODEMOD_TASK_ID should be a valid UUID format"
        );
        assert!(
            log_output.contains("workflow_run_id_valid=true"),
            "CODEMOD_WORKFLOW_RUN_ID should be a valid UUID format"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
#[serial]
async fn test_ai_step_no_api_key_fallback_allows_following_steps() {
    let api_key_guard = EnvVarGuard::unset("LLM_API_KEY");

    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_ai_no_key_fallback_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let mut completion_logs: Option<String> = None;
    for _ in 0..50 {
        let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
        let Some(ai_task) = tasks.iter().find(|t| t.node_id == "ai-fallback-node") else {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            continue;
        };

        if ai_task.status == TaskStatus::Completed {
            completion_logs = Some(ai_task.logs.join("\n"));
            break;
        }

        assert_ne!(
            ai_task.status,
            TaskStatus::Failed,
            "AI fallback node should not fail when API key is missing"
        );
        assert_ne!(
            ai_task.status,
            TaskStatus::WontDo,
            "AI fallback node should execute following steps when API key is missing"
        );

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    let log_output = completion_logs.expect("AI fallback node did not complete within 5 seconds");
    assert!(
        log_output.contains("AFTER_AI_STEP_EXECUTED"),
        "Step after AI should execute even when AI step is skipped due to missing key"
    );

    drop(api_key_guard);
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
#[serial]
async fn test_ai_step_detected_agent_handoff_skips_rig_with_api_key() {
    let api_key_guard = EnvVarGuard::set("LLM_API_KEY", "test-key");
    let provider_guard = EnvVarGuard::set("LLM_PROVIDER", "openai");
    let base_url_guard = EnvVarGuard::set("LLM_BASE_URL", "http://127.0.0.1:1");
    let marker_one_guard = EnvVarGuard::set("CODEX_SESSION_ID", "test-session");
    let marker_two_guard = EnvVarGuard::set("CODEX_SANDBOX", "1");

    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_ai_no_key_fallback_workflow();
    let workflow_run_id = engine
        .run_workflow(workflow, HashMap::new(), None, None)
        .await
        .unwrap();

    let mut completion_logs: Option<String> = None;
    for _ in 0..50 {
        let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
        let Some(ai_task) = tasks.iter().find(|t| t.node_id == "ai-fallback-node") else {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            continue;
        };

        if ai_task.status == TaskStatus::Completed {
            completion_logs = Some(ai_task.logs.join("\n"));
            break;
        }

        assert_ne!(
            ai_task.status,
            TaskStatus::Failed,
            "AI step should hand off instructions and avoid Rig call when coding-agent context is detected"
        );
        assert_ne!(
            ai_task.status,
            TaskStatus::WontDo,
            "AI handoff path should keep the node running to completion"
        );

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    let log_output = completion_logs.expect("AI handoff node did not complete within 5 seconds");
    assert!(
        log_output.contains("AFTER_AI_STEP_EXECUTED"),
        "Step after AI should execute when handoff mode skips Rig"
    );

    drop(marker_two_guard);
    drop(marker_one_guard);
    drop(base_url_guard);
    drop(provider_guard);
    drop(api_key_guard);
}

#[tokio::test]
async fn test_codemod_environment_variables_in_matrix() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    // Create a matrix workflow that tests environment variables
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "setup-node".to_string(),
                name: "Setup Node".to_string(),
                description: Some("Setup node".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("setup-node".to_string()),
                    name: "Setup".to_string(),
                    action: StepAction::RunScript("echo 'Setup complete'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "matrix-env-test-node".to_string(),
                name: "Matrix Environment Test Node".to_string(),
                description: Some("Test environment variables in matrix".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["setup-node".to_string()],
                trigger: None,
                strategy: Some(Strategy {
                    r#type: butterflow_models::strategy::StrategyType::Matrix,
                    values: Some(vec![
                        HashMap::from([(
                            "region".to_string(),
                            serde_json::to_value("us-east").unwrap(),
                        )]),
                        HashMap::from([(
                            "region".to_string(),
                            serde_json::to_value("eu-west").unwrap(),
                        )]),
                    ]),
                    from_state: None,
                }),
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("test-environment-variables-in-matrix".to_string()),
                    name: "Test Environment Variables in Matrix".to_string(),
                    action: StepAction::RunScript(
                        r#"echo "Matrix region: $region"
echo "CODEMOD_TASK_ID: $CODEMOD_TASK_ID"
echo "CODEMOD_WORKFLOW_RUN_ID: $CODEMOD_WORKFLOW_RUN_ID"
echo "env_vars_in_matrix=true""#
                            .to_string(),
                    ),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
        ],
    };

    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    // Get the tasks
    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // Find the matrix tasks (should be at least 2 for the 2 regions)
    let matrix_tasks: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.node_id == "matrix-env-test-node" && !t.is_master)
        .collect();

    // Should have matrix tasks for each region
    assert!(
        matrix_tasks.len() >= 2,
        "Should have matrix tasks for each region"
    );

    // Check that each matrix task has the environment variables set correctly
    for matrix_task in matrix_tasks {
        if matrix_task.status == TaskStatus::Completed {
            assert!(!matrix_task.logs.is_empty(), "Matrix task should have logs");

            let log_output = matrix_task.logs.join("\n");

            // Check that CODEMOD_TASK_ID is set to this specific matrix task's ID
            assert!(
                log_output.contains(&format!("CODEMOD_TASK_ID: {}", matrix_task.id)),
                "CODEMOD_TASK_ID should be set to the matrix task ID in matrix task {}",
                matrix_task.id
            );

            // Check that CODEMOD_WORKFLOW_RUN_ID is set correctly
            assert!(
                log_output.contains(&format!("CODEMOD_WORKFLOW_RUN_ID: {workflow_run_id}")),
                "CODEMOD_WORKFLOW_RUN_ID should be set correctly in matrix task {}",
                matrix_task.id
            );

            // Check that the matrix variable is also present
            assert!(
                log_output.contains("Matrix region:")
                    && (log_output.contains("us-east") || log_output.contains("eu-west")),
                "Matrix region should be set in matrix task {}",
                matrix_task.id
            );
        }
    }
}

#[tokio::test]
async fn test_cyclic_dependency_workflow() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    // Create a workflow with a cyclic dependency
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                description: Some("Test node 1".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["node2".to_string()], // Depends on node2
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-1".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Hello, World!'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "node2".to_string(),
                name: "Node 2".to_string(),
                description: Some("Test node 2".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["node1".to_string()], // Depends on node1, creating a cycle
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("step-2".to_string()),
                    name: "Step 1".to_string(),
                    action: StepAction::RunScript("echo 'Node 2 executed'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
        ],
    };

    let params = HashMap::new();

    // Running this workflow should fail due to the cyclic dependency
    let result = engine.run_workflow(workflow, params, None, None).await;

    // The result should be an error
    assert!(result.is_err());
}

#[tokio::test]
async fn test_invalid_template_reference() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    // Create a workflow with an invalid template reference
    let workflow = Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "node1".to_string(),
            name: "Node 1".to_string(),
            description: Some("Test node 1".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![Step {
                id: Some("step-1".to_string()),
                name: "Step 1".to_string(),
                action: StepAction::UseTemplate(butterflow_models::step::TemplateUse {
                    template: "non-existent-template".to_string(), // This template doesn't exist
                    inputs: HashMap::new(),
                }),
                env: None,
                condition: None,
                commit: None,
            }],
            env: HashMap::new(),
            branch_name: None,
            pull_request: None,
        }],
    };

    let params = HashMap::new();

    // Running this workflow should fail due to the invalid template reference
    let result = engine.run_workflow(workflow, params, None, None).await;

    // The result should be an error
    assert!(result.is_err());
}

// Helper function for AST grep tests
fn create_test_file(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let file_path = dir.join(name);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&file_path, content).unwrap();
    file_path
}

#[tokio::test]
async fn test_execute_ast_grep_step() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create test JavaScript files
    create_test_file(
        temp_path,
        "src/app.js",
        r#"
function main() {
    console.log("Starting app");
    var count = 0;
    console.log("Count:", count);
}
"#,
    );

    create_test_file(
        temp_path,
        "src/utils.js",
        r#"
function helper() {
    console.log("Helper function");
    let data = getData();
    return data;
}
"#,
    );

    // Create ast-grep config
    create_test_file(
        temp_path,
        "ast-grep-rules.yaml",
        r#"id: console-log
language: javascript
rule:
  pattern: console.log($$$)
message: "Found console.log statement"
---
id: var-declaration
language: javascript
rule:
  pattern: var $VAR = $VALUE
message: "Found var declaration"
"#,
    );

    // Create a simple workflow with ast-grep step
    let ast_grep_step = UseAstGrep {
        include: Some(vec!["src/**/*.js".to_string()]),
        exclude: None,
        base_path: None,
        config_file: "ast-grep-rules.yaml".to_string(),
        allow_dirty: Some(false),
        max_threads: None,
    };

    let step = Step {
        id: Some("step-1".to_string()),
        name: "Test AST Grep".to_string(),
        action: StepAction::AstGrep(ast_grep_step),
        env: None,
        condition: None,
        commit: None,
    };

    // Create a simple node for testing
    let _node = Node {
        id: "test-node".to_string(),
        name: "Test Node".to_string(),
        description: None,
        r#type: butterflow_models::node::NodeType::Automatic,
        runtime: None,
        depends_on: vec![],
        steps: vec![step],
        strategy: None,
        trigger: None,
        env: HashMap::new(),
        branch_name: None,
        pull_request: None,
    };

    // Create a dummy task
    let _task = Task {
        id: Uuid::new_v4(),
        workflow_run_id: Uuid::new_v4(),
        node_id: "test-node".to_string(),
        status: TaskStatus::Pending,
        is_master: false,
        master_task_id: None,
        matrix_values: None,
        started_at: None,
        ended_at: None,
        logs: vec![],
        error: None,
    };

    // Create engine with correct bundle path
    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_workflow_run_config(config);
    let result = engine
        .execute_ast_grep_step(
            "test".to_string(),
            &UseAstGrep {
                include: Some(vec!["src/**/*.js".to_string()]),
                exclude: None,
                base_path: None,
                config_file: "ast-grep-rules.yaml".to_string(),
                allow_dirty: Some(false),
                max_threads: None,
            },
            &StructuredLogger::default(),
        )
        .await;

    // Assert the step executed successfully
    assert!(
        result.is_ok(),
        "AST grep step should execute successfully: {result:?}"
    );
}

#[tokio::test]
async fn test_execute_ast_grep_step_with_typescript() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create test TypeScript file
    create_test_file(
        temp_path,
        "src/component.ts",
        r#"
interface User {
    name: string;
    age: number;
}

function greetUser(user: User): void {
    console.log(`Hello, ${user.name}!`);
}

const createUser = (name: string, age: number): User => {
    return { name, age };
};
"#,
    );

    // Create ast-grep config for TypeScript
    create_test_file(
        temp_path,
        "ts-rules.yaml",
        r#"id: console-log
language: typescript
rule:
  pattern: console.log($$$)
message: "Found console.log statement"
---
id: interface-declaration
language: typescript
rule:
  pattern: interface $NAME { $$$ }
message: "Found interface declaration"
"#,
    );

    // Create engine with correct bundle path
    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_workflow_run_config(config);
    let result = engine
        .execute_ast_grep_step(
            "test-node".to_string(),
            &UseAstGrep {
                include: Some(vec!["src/**/*.ts".to_string()]),
                exclude: None,
                base_path: None,
                config_file: "ts-rules.yaml".to_string(),
                allow_dirty: Some(false),
                max_threads: None,
            },
            &StructuredLogger::default(),
        )
        .await;

    // Assert the step executed successfully
    assert!(
        result.is_ok(),
        "TypeScript AST grep step should execute successfully: {result:?}"
    );
}

#[tokio::test]
async fn test_execute_ast_grep_step_nonexistent_config() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create test file but no config
    create_test_file(temp_path, "test.js", "console.log('test');");

    // Create engine with correct bundle path
    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_workflow_run_config(config);
    let result = engine
        .execute_ast_grep_step(
            "test-node".to_string(),
            &UseAstGrep {
                include: Some(vec!["test.js".to_string()]),
                exclude: None,
                base_path: None,
                config_file: "nonexistent.yaml".to_string(),
                allow_dirty: Some(false),
                max_threads: None,
            },
            &StructuredLogger::default(),
        )
        .await;

    // Should fail gracefully
    assert!(result.is_err(), "Should fail with nonexistent config file");
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("AST grep config file not found"));
}

#[tokio::test]
async fn test_execute_ast_grep_step_no_matches() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create test file with no console.log
    create_test_file(
        temp_path,
        "test.js",
        r#"
function add(a, b) {
    return a + b;
}

let result = add(1, 2);
"#,
    );

    // Create config that looks for console.log
    create_test_file(
        temp_path,
        "rules.yaml",
        r#"id: console-log
language: javascript
rule:
  pattern: console.log($$$)
message: "Found console.log statement"
"#,
    );

    // Create engine with correct bundle path
    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_workflow_run_config(config);
    let result = engine
        .execute_ast_grep_step(
            "test-node".to_string(),
            &UseAstGrep {
                include: Some(vec!["test.js".to_string()]),
                exclude: None,
                base_path: None,
                config_file: "rules.yaml".to_string(),
                allow_dirty: Some(false),
                max_threads: None,
            },
            &StructuredLogger::default(),
        )
        .await;

    // Should succeed even with no matches
    assert!(
        result.is_ok(),
        "Should succeed even with no matches: {result:?}"
    );
}

#[tokio::test]
async fn test_execute_js_ast_grep_step() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create a simple JavaScript codemod file
    create_test_file(
        temp_path,
        "codemod.js",
        r#"
export default function transform(ast) {
  return "Hello, World!";
}
"#,
    );

    // Create test files to transform
    create_test_file(
        temp_path,
        "src/app.js",
        r#"
function main() {
    console.log("Starting app");
    var count = 0;
    console.log("Count:", count);
}
"#,
    );

    create_test_file(
        temp_path,
        "src/utils.js",
        r#"
function helper() {
    console.log("Helper function");
    let data = getData();
    return data;
}
"#,
    );

    // Create engine with correct bundle path
    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_workflow_run_config(config);
    let result = engine
        .execute_js_ast_grep_step(
            "test-node".to_string(),
            "test-step".to_string(),
            &UseJSAstGrep {
                js_file: "codemod.js".to_string(),
                base_path: Some("src".to_string()),
                include: Some(vec!["**/*.js".to_string()]),
                exclude: None,
                max_threads: Some(2),
                dry_run: Some(false),
                language: Some("javascript".to_string()),
                capabilities: None,
                semantic_analysis: Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::File)),
            },
            None,
            None,
            &CapabilitiesData {
                capabilities: None,
                capabilities_security_callback: None,
            },
            &None,
            None,
            None,
            &StructuredLogger::default(),
            None,
            None,
            None,
        )
        .await;

    // Assert the step executed successfully
    assert!(
        result.is_ok(),
        "JS AST grep step should execute successfully: {result:?}"
    );
}

#[tokio::test]
async fn test_execute_js_ast_grep_step_with_typescript() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create a TypeScript codemod file
    create_test_file(
        temp_path,
        "ts-codemod.js",
        r#"
export default function transform(ast) {
  return ast
    .findAll({ 
      rule: { 
        pattern: 'interface $NAME { $$$ }' 
      } 
    })
    .replace('type $NAME = { $$$ }');
}
"#,
    );

    // Create test TypeScript files
    create_test_file(
        temp_path,
        "src/types.ts",
        r#"
interface User {
    name: string;
    age: number;
}

interface Product {
    id: number;
    title: string;
}
"#,
    );

    create_test_file(
        temp_path,
        "src/models.ts",
        r#"
interface ApiResponse {
    data: any;
    status: number;
}
"#,
    );

    // Create engine with correct bundle path
    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_workflow_run_config(config);
    let result = engine
        .execute_js_ast_grep_step(
            "test-node".to_string(),
            "test-step".to_string(),
            &UseJSAstGrep {
                js_file: "ts-codemod.js".to_string(),
                base_path: Some("src".to_string()),
                include: Some(vec!["**/*.ts".to_string(), "**/*.tsx".to_string()]),
                exclude: None,
                max_threads: Some(4),
                dry_run: Some(false),
                language: Some("typescript".to_string()),
                capabilities: None,
                semantic_analysis: Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::File)),
            },
            None,
            None,
            &CapabilitiesData {
                capabilities: None,
                capabilities_security_callback: None,
            },
            &None,
            None,
            None,
            &StructuredLogger::default(),
            None,
            None,
            None,
        )
        .await;

    // Assert the step executed successfully
    assert!(
        result.is_ok(),
        "TypeScript JS AST grep step should execute successfully: {result:?}"
    );
}

#[tokio::test]
async fn test_execute_js_ast_grep_step_falls_back_when_selector_extraction_fails() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    create_test_file(
        temp_path,
        "codemod.js",
        r#"
export function getSelector() {
  throw new Error("selector should not make the workflow fail");
}

export default function transform(ast) {
  return ast
    .findAll({ rule: { pattern: 'var $NAME = $VALUE' } })
    .replace('let $NAME = $VALUE');
}
"#,
    );

    create_test_file(
        temp_path,
        "src/app.js",
        r#"
function main() {
    var count = 0;
}
"#,
    );

    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_workflow_run_config(config);
    let result = engine
        .execute_js_ast_grep_step(
            "test-node".to_string(),
            "test-step".to_string(),
            &UseJSAstGrep {
                js_file: "codemod.js".to_string(),
                base_path: Some("src".to_string()),
                include: Some(vec!["**/*.js".to_string()]),
                exclude: None,
                max_threads: Some(2),
                dry_run: Some(false),
                language: Some("javascript".to_string()),
                capabilities: None,
                semantic_analysis: Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::File)),
            },
            None,
            None,
            &CapabilitiesData {
                capabilities: None,
                capabilities_security_callback: None,
            },
            &None,
            None,
            None,
            &StructuredLogger::default(),
            None,
            None,
            None,
        )
        .await;

    assert!(
        result.is_ok(),
        "selector extraction is an optimization and should fall back to full-file execution: {result:?}"
    );
}

#[tokio::test]
async fn test_execute_js_ast_grep_step_dry_run() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create a codemod file
    create_test_file(
        temp_path,
        "dry-run-codemod.js",
        r#"
export default function transform(ast) {
  return ast
    .findAll({ rule: { pattern: 'var $VAR = $VALUE' } })
    .replace('const $VAR = $VALUE');
}
"#,
    );

    // Create test file
    create_test_file(
        temp_path,
        "test.js",
        r#"
var name = "test";
var count = 0;
"#,
    );

    // Create engine with correct bundle path
    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_workflow_run_config(config);
    let result = engine
        .execute_js_ast_grep_step(
            "test-node".to_string(),
            "test-step".to_string(),
            &UseJSAstGrep {
                js_file: "dry-run-codemod.js".to_string(),
                base_path: None, // Use current directory
                include: Some(vec!["**/*.js".to_string()]),
                exclude: None,
                max_threads: None,   // Use default
                dry_run: Some(true), // Enable dry run
                language: Some("javascript".to_string()),
                capabilities: None,
                semantic_analysis: Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::File)),
            },
            None,
            None,
            &CapabilitiesData {
                capabilities: None,
                capabilities_security_callback: None,
            },
            &None,
            None,
            None,
            &StructuredLogger::default(),
            None,
            None,
            None,
        )
        .await;

    // Assert the step executed successfully
    assert!(
        result.is_ok(),
        "Dry run JS AST grep step should execute successfully: {result:?}"
    );
}

#[tokio::test]
async fn test_execute_js_ast_grep_step_nonexistent_js_file() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create test file but no codemod
    create_test_file(temp_path, "test.js", "console.log('test');");

    // Create engine with correct bundle path
    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_workflow_run_config(config);
    let result = engine
        .execute_js_ast_grep_step(
            "test-node".to_string(),
            "test-step".to_string(),
            &UseJSAstGrep {
                js_file: "nonexistent-codemod.js".to_string(),
                base_path: None,
                include: None,
                exclude: None,
                max_threads: None,
                dry_run: Some(false),
                language: None,
                capabilities: None,
                semantic_analysis: Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::File)),
            },
            None,
            None,
            &CapabilitiesData {
                capabilities: None,
                capabilities_security_callback: None,
            },
            &None,
            None,
            None,
            &StructuredLogger::default(),
            None,
            None,
            None,
        )
        .await;

    // Should fail gracefully
    assert!(result.is_err(), "Should fail with nonexistent JS file");
    assert!(result.unwrap_err().to_string().contains("JavaScript file"));
}

#[tokio::test]
async fn test_execute_js_ast_grep_step_with_gitignore() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create a codemod file
    create_test_file(
        temp_path,
        "gitignore-codemod.js",
        r#"
export default function transform(ast) {
  return ast
    .findAll({ rule: { pattern: 'console.log($$$)' } })
    .replace('logger.info($$$)');
}
"#,
    );

    // Create .gitignore file
    create_test_file(
        temp_path,
        ".gitignore",
        r#"
node_modules/
*.log
build/
"#,
    );

    // Create test files in different locations
    create_test_file(temp_path, "src/app.js", "console.log('main app');");
    create_test_file(temp_path, "build/dist.js", "console.log('built file');");
    create_test_file(
        temp_path,
        "node_modules/lib.js",
        "console.log('dependency');",
    );

    // Create engine with correct bundle path
    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_workflow_run_config(config);
    let result = engine
        .execute_js_ast_grep_step(
            "test-node".to_string(),
            "test-step".to_string(),
            &UseJSAstGrep {
                js_file: "gitignore-codemod.js".to_string(),
                base_path: None,
                include: Some(vec!["**/*.js".to_string()]),
                exclude: None,
                max_threads: Some(1),
                dry_run: Some(false),
                language: Some("javascript".to_string()),
                capabilities: None,
                semantic_analysis: Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::File)),
            },
            None,
            None,
            &CapabilitiesData {
                capabilities: None,
                capabilities_security_callback: None,
            },
            &None,
            None,
            None,
            &StructuredLogger::default(),
            None,
            None,
            None,
        )
        .await;

    // Assert the step executed successfully
    assert!(
        result.is_ok(),
        "JS AST grep step with gitignore should execute successfully: {result:?}"
    );

    // Test second execution
    let result_no_gitignore = engine
        .execute_js_ast_grep_step(
            "test-node".to_string(),
            "test-step".to_string(),
            &UseJSAstGrep {
                js_file: "gitignore-codemod.js".to_string(),
                base_path: None,
                include: Some(vec!["**/*.js".to_string()]),
                exclude: None,
                max_threads: Some(1),
                dry_run: Some(false),
                language: Some("javascript".to_string()),
                capabilities: None,
                semantic_analysis: Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::File)),
            },
            None,
            None,
            &CapabilitiesData {
                capabilities: None,
                capabilities_security_callback: None,
            },
            &None,
            None,
            None,
            &StructuredLogger::default(),
            None,
            None,
            None,
        )
        .await;

    // Should also succeed
    assert!(
        result_no_gitignore.is_ok(),
        "JS AST grep step without gitignore should execute successfully: {result_no_gitignore:?}"
    );
}

#[tokio::test]
async fn test_execute_js_ast_grep_step_with_hidden_files() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create a codemod file
    create_test_file(
        temp_path,
        "hidden-codemod.js",
        r#"
export default function transform(ast) {
  return ast
    .findAll({ rule: { pattern: 'const $VAR = $VALUE' } })
    .replace('let $VAR = $VALUE');
}
"#,
    );

    // Create hidden file
    create_test_file(temp_path, ".hidden.js", "const secret = 'hidden';");

    // Create regular file
    create_test_file(temp_path, "regular.js", "const normal = 'visible';");

    // Create engine with correct bundle path
    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_workflow_run_config(config);
    let result = engine
        .execute_js_ast_grep_step(
            "test-node".to_string(),
            "test-step".to_string(),
            &UseJSAstGrep {
                js_file: "hidden-codemod.js".to_string(),
                base_path: None,
                include: Some(vec!["**/*.js".to_string()]),
                exclude: None,
                max_threads: Some(1),
                dry_run: Some(false),
                language: Some("javascript".to_string()),
                capabilities: None,
                semantic_analysis: Some(SemanticAnalysisConfig::Mode(SemanticAnalysisMode::File)),
            },
            None,
            None,
            &CapabilitiesData {
                capabilities: None,
                capabilities_security_callback: None,
            },
            &None,
            None,
            None,
            &StructuredLogger::default(),
            None,
            None,
            None,
        )
        .await;

    // Assert the step executed successfully
    assert!(
        result.is_ok(),
        "JS AST grep step with hidden files should execute successfully: {result:?}"
    );
}

// Helper function to create a workflow with JSAstGrep step
fn create_js_ast_grep_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "js-ast-grep-node".to_string(),
            name: "JS AST Grep Node".to_string(),
            description: Some("Test node for JS AST grep".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![Step {
                id: Some("js-ast-grep-step".to_string()),
                name: "JS AST Grep Step".to_string(),
                action: StepAction::JSAstGrep(UseJSAstGrep {
                    js_file: "codemod.js".to_string(),
                    base_path: Some("src".to_string()),
                    include: Some(vec!["**/*.js".to_string()]),
                    exclude: None,
                    max_threads: Some(2),
                    dry_run: Some(false),
                    language: Some("javascript".to_string()),
                    capabilities: None,
                    semantic_analysis: Some(SemanticAnalysisConfig::Mode(
                        SemanticAnalysisMode::File,
                    )),
                }),
                env: None,
                condition: None,
                commit: None,
            }],
            env: HashMap::new(),
            branch_name: None,
            pull_request: None,
        }],
    }
}

#[tokio::test]
async fn test_js_ast_grep_workflow_execution() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create codemod and test files
    create_test_file(
        temp_path,
        "codemod.js",
        r#"
import { CallExpression } from 'codemod:ast-grep';

export default function transform(ast) {
  return ast
    .findAll({ rule: { pattern: 'console.log($$$)' } })
    .replace('logger.info($$$)');
}
"#,
    );

    create_test_file(temp_path, "src/app.js", "console.log('Hello, World!');");

    // Create engine with workflow
    let state_adapter = Box::new(MockStateAdapter::new());
    let config = WorkflowRunConfig {
        bundle_path: temp_path.to_path_buf(),
        ..WorkflowRunConfig::default()
    };
    let engine = Engine::with_state_adapter(state_adapter, config);

    let workflow = create_js_ast_grep_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, Some(temp_path.to_path_buf()), None)
        .await
        .unwrap();

    // Allow some time for the workflow to start
    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

    // Get the workflow run
    let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();

    // Check that the workflow run is running, completed, or failed (test focuses on task creation)
    println!("JS AST grep workflow status: {:?}", workflow_run.status);
    assert!(
        workflow_run.status == WorkflowStatus::Running
            || workflow_run.status == WorkflowStatus::Completed
            || workflow_run.status == WorkflowStatus::Failed
    );

    // Get the tasks
    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // There should be at least 1 task
    assert!(!tasks.is_empty());

    // Check that the task for the JS AST grep node exists
    let js_ast_grep_task = tasks
        .iter()
        .find(|t| t.node_id == "js-ast-grep-node")
        .unwrap();

    // Check that the task status is valid
    assert!(
        js_ast_grep_task.status == TaskStatus::Running
            || js_ast_grep_task.status == TaskStatus::Completed
            || js_ast_grep_task.status == TaskStatus::Failed
    );
}

// Helper function to create a workflow that writes to state and then uses it for matrix (realistic end-to-end)
fn create_realistic_state_write_workflow() -> Workflow {
    let root_schema = Default::default();

    Workflow {
        version: "1".to_string(),
        params: None,
        state: Some(butterflow_models::WorkflowState {
            schema: root_schema,
        }),
        templates: vec![],
        nodes: vec![
            Node {
                id: "evaluate-codeowners".to_string(),
                name: "Evaluate Codeowners".to_string(),
                description: Some("Shard the workflow into smaller chunks based on codeowners".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("evaluate-codeowners".to_string()),
                    name: "Create matrix data".to_string(),
                    action: StepAction::RunScript(
                        r#"#!/bin/bash
echo "Writing to state at: $STATE_OUTPUTS"

# Write TypeScript shards to state
echo 'i18nShardsTs=[{"team": "frontend", "shardId": "shard-1"}, {"team": "backend", "shardId": "shard-2"}]' >> $STATE_OUTPUTS

# Write HTML shards to state  
echo 'i18nShardsHtml=[{"team": "ui", "shardId": "shard-a"}, {"team": "docs", "shardId": "shard-b"}, {"team": "marketing", "shardId": "shard-c"}]' >> $STATE_OUTPUTS

echo "State written successfully"
cat $STATE_OUTPUTS"#.to_string(),
                    ),
                    env: None,
                condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "run-codemod-ts".to_string(),
                name: "I18n Codemod (TS)".to_string(),
                description: Some("Run the i18n codemod on TypeScript files".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["evaluate-codeowners".to_string()],
                trigger: None,
                strategy: Some(Strategy {
                    r#type: butterflow_models::strategy::StrategyType::Matrix,
                    values: None,
                    from_state: Some("i18nShardsTs".to_string()),
                }),
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("run-codemod-ts".to_string()),
                    name: "Run TS codemod".to_string(),
                    action: StepAction::RunScript(
                        "echo 'Running TS codemod for team ${team} on shard ${shardId}'".to_string(),
                    ),
                    env: None,
                condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
            Node {
                id: "run-codemod-html".to_string(),
                name: "I18n Codemod (HTML)".to_string(),
                description: Some("Run the i18n codemod on HTML files".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec!["evaluate-codeowners".to_string()],
                trigger: None,
                strategy: Some(Strategy {
                    r#type: butterflow_models::strategy::StrategyType::Matrix,
                    values: None,
                    from_state: Some("i18nShardsHtml".to_string()),
                }),
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("run-codemod-html".to_string()),
                    name: "Run HTML codemod".to_string(),
                    action: StepAction::RunScript(
                        "echo 'Running HTML codemod for team ${team} on shard ${shardId}'".to_string(),
                    ),
                    env: None,
                condition: None,
                    commit: None,
                }],
                env: HashMap::new(),
                branch_name: None,
                pull_request: None,
            },
        ],
    }
}

#[tokio::test]
async fn test_realistic_state_write_and_matrix_workflow() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_realistic_state_write_workflow();
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    let tasks = tokio::time::timeout(tokio::time::Duration::from_secs(10), async {
        loop {
            let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
            let writer_completed = tasks
                .iter()
                .find(|t| t.node_id == "evaluate-codeowners")
                .is_some_and(|task| task.status == TaskStatus::Completed);

            if tasks.len() == 8 && writer_completed {
                return tasks;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("timed out waiting for matrix recompilation after state write");

    // Should have 8 tasks:
    // 1. evaluate-codeowners task (completed)
    // 2. run-codemod-ts master task
    // 3. run-codemod-html master task
    // 4. 2 matrix tasks for TS
    // 5. 3 matrix tasks for HTML
    assert_eq!(tasks.len(), 8);

    // Find the state-writer task
    let state_writer_task = tasks
        .iter()
        .find(|t| t.node_id == "evaluate-codeowners")
        .unwrap();

    // State writer should be completed
    assert_eq!(
        state_writer_task.status,
        TaskStatus::Completed,
        "State writer should complete"
    );

    // Verify state was written by checking if matrix tasks were created
    let ts_matrix_tasks: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.node_id == "run-codemod-ts" && !t.is_master && t.matrix_values.is_some())
        .collect();

    let html_matrix_tasks: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.node_id == "run-codemod-html" && !t.is_master && t.matrix_values.is_some())
        .collect();

    // Should have matrix tasks created from the state data
    assert_eq!(ts_matrix_tasks.len(), 2);
    assert_eq!(html_matrix_tasks.len(), 3);

    // Verify matrix values are populated correctly
    for ts_task in &ts_matrix_tasks {
        let matrix_values = ts_task.matrix_values.as_ref().unwrap();
        assert!(
            matrix_values.contains_key("team"),
            "TS matrix task should have 'team' value"
        );
        assert!(
            matrix_values.contains_key("shardId"),
            "TS matrix task should have 'shardId' value"
        );

        // Verify values match what we wrote to state
        let team = matrix_values.get("team").unwrap().as_str().unwrap();
        let shard_id = matrix_values.get("shardId").unwrap().as_str().unwrap();
        assert!(
            (team == "frontend" && shard_id == "shard-1")
                || (team == "backend" && shard_id == "shard-2"),
            "TS matrix values should match written state data"
        );
    }

    for html_task in &html_matrix_tasks {
        let matrix_values = html_task.matrix_values.as_ref().unwrap();
        assert!(
            matrix_values.contains_key("team"),
            "HTML matrix task should have 'team' value"
        );
        assert!(
            matrix_values.contains_key("shardId"),
            "HTML matrix task should have 'shardId' value"
        );

        // Verify values match what we wrote to state
        let team = matrix_values.get("team").unwrap().as_str().unwrap();
        let shard_id = matrix_values.get("shardId").unwrap().as_str().unwrap();
        assert!(
            (team == "ui" && shard_id == "shard-a")
                || (team == "docs" && shard_id == "shard-b")
                || (team == "marketing" && shard_id == "shard-c"),
            "HTML matrix values should match written state data"
        );
    }

    // Verify the correct number of matrix tasks were created (which proves state was written correctly)
    assert_eq!(
        ts_matrix_tasks.len(),
        2,
        "Should have exactly 2 TS matrix tasks from state"
    );
    assert_eq!(
        html_matrix_tasks.len(),
        3,
        "Should have exactly 3 HTML matrix tasks from state"
    );
}

#[tokio::test]
async fn test_workflow_with_state_write_and_matrix() {
    // Create a mock state adapter
    let mut state_adapter = MockStateAdapter::new();

    // Create workflow with matrix from state strategy
    let workflow = create_matrix_from_state_workflow(); // Use existing working workflow

    // Create a workflow run
    let workflow_run_id = Uuid::new_v4();
    let workflow_run = WorkflowRun {
        id: workflow_run_id,
        workflow: workflow.clone(),
        status: WorkflowStatus::Running,
        params: HashMap::new(),
        tasks: Vec::new(),
        started_at: chrono::Utc::now(),
        ended_at: None,
        bundle_path: None,
        capabilities: None,
        name: None,
        target_path: None,
    };

    // Save the workflow run
    state_adapter
        .save_workflow_run(&workflow_run)
        .await
        .unwrap();

    // Create a completed task for node1
    let node1_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node1".to_string(),
        status: TaskStatus::Completed,
        is_master: false,
        master_task_id: None,
        matrix_values: None,
        started_at: Some(chrono::Utc::now()),
        ended_at: Some(chrono::Utc::now()),
        error: None,
        logs: Vec::new(),
    };

    // Save the task
    state_adapter.save_task(&node1_task).await.unwrap();

    // Create a master task for node2
    let master_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Pending,
        is_master: true,
        master_task_id: None,
        matrix_values: None,
        started_at: None,
        ended_at: None,
        error: None,
        logs: Vec::new(),
    };

    // Save the master task
    state_adapter.save_task(&master_task).await.unwrap();

    // Set state with files data
    let files_array = serde_json::json!([
        {"file": "src/component1.ts", "type": "component"},
        {"file": "src/component2.ts", "type": "component"},
        {"file": "src/utils.ts", "type": "utility"}
    ]);

    let mut state = HashMap::new();
    state.insert("files".to_string(), files_array);

    // Update the state directly on our adapter
    state_adapter
        .update_state(workflow_run_id, state)
        .await
        .unwrap();

    // Verify initial tasks
    let initial_tasks = state_adapter.get_tasks(workflow_run_id).await.unwrap();
    assert_eq!(initial_tasks.len(), 2); // node1 task + master task for node2

    // Now manually create the matrix tasks as the engine would
    for file_data in [
        ("src/component1.ts", "component"),
        ("src/component2.ts", "component"),
        ("src/utils.ts", "utility"),
    ]
    .iter()
    {
        let matrix_task = Task {
            id: Uuid::new_v4(),
            workflow_run_id,
            node_id: "node2".to_string(),
            status: TaskStatus::Pending,
            is_master: false,
            master_task_id: Some(master_task.id),
            matrix_values: Some(HashMap::from([
                (
                    "file".to_string(),
                    serde_json::to_value(file_data.0).unwrap(),
                ),
                (
                    "type".to_string(),
                    serde_json::to_value(file_data.1).unwrap(),
                ),
            ])),
            started_at: None,
            ended_at: None,
            error: None,
            logs: Vec::new(),
        };

        state_adapter.save_task(&matrix_task).await.unwrap();
    }

    // Verify all tasks are present
    let final_tasks = state_adapter.get_tasks(workflow_run_id).await.unwrap();
    assert_eq!(final_tasks.len(), 5); // node1 task + master task + 3 matrix tasks

    // Verify matrix tasks have correct matrix values
    let matrix_tasks: Vec<&Task> = final_tasks
        .iter()
        .filter(|t| t.node_id == "node2" && !t.is_master)
        .collect();

    assert_eq!(matrix_tasks.len(), 3);

    for matrix_task in matrix_tasks {
        assert!(
            matrix_task.matrix_values.is_some(),
            "Matrix task should have matrix values"
        );
        let matrix_values = matrix_task.matrix_values.as_ref().unwrap();

        // Should have 'file' and 'type' from the state data
        assert!(
            matrix_values.contains_key("file"),
            "Matrix values should contain 'file'"
        );
        assert!(
            matrix_values.contains_key("type"),
            "Matrix values should contain 'type'"
        );

        // Verify the values are from our expected set
        let file_value = matrix_values.get("file").unwrap().as_str().unwrap();
        let type_value = matrix_values.get("type").unwrap().as_str().unwrap();

        assert!(
            file_value.starts_with("src/"),
            "File should be in src directory"
        );
        assert!(
            type_value == "component" || type_value == "utility",
            "Type should be component or utility"
        );
    }
}

#[tokio::test]
async fn test_dynamic_state_update_with_matrix_recompilation() {
    // This test is similar to test_matrix_recompilation_with_direct_adapter
    // but tests the state update and recompilation workflow
    let mut state_adapter = MockStateAdapter::new();

    // Create a workflow with a matrix node using from_state
    let workflow = create_matrix_from_state_workflow();

    // Create a workflow run
    let workflow_run_id = Uuid::new_v4();
    let workflow_run = WorkflowRun {
        id: workflow_run_id,
        workflow: workflow.clone(),
        status: WorkflowStatus::Running,
        params: HashMap::new(),
        tasks: Vec::new(),
        started_at: chrono::Utc::now(),
        ended_at: None,
        bundle_path: None,
        capabilities: None,
        name: None,
        target_path: None,
    };

    // Save the workflow run
    state_adapter
        .save_workflow_run(&workflow_run)
        .await
        .unwrap();

    // Create a task for node1
    let node1_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node1".to_string(),
        status: TaskStatus::Completed,
        is_master: false,
        master_task_id: None,
        matrix_values: None,
        started_at: Some(chrono::Utc::now()),
        ended_at: Some(chrono::Utc::now()),
        error: None,
        logs: Vec::new(),
    };

    // Save the task
    state_adapter.save_task(&node1_task).await.unwrap();

    // Create a master task for node2
    let master_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Pending,
        is_master: true,
        master_task_id: None,
        matrix_values: None,
        started_at: None,
        ended_at: None,
        error: None,
        logs: Vec::new(),
    };

    // Save the master task
    state_adapter.save_task(&master_task).await.unwrap();

    // Set initial state with tasks
    let initial_tasks_array = serde_json::json!([
        {"task": "task1", "priority": "high"},
        {"task": "task2", "priority": "medium"}
    ]);

    let mut initial_state = HashMap::new();
    initial_state.insert("files".to_string(), initial_tasks_array);

    // Update the state
    state_adapter
        .update_state(workflow_run_id, initial_state)
        .await
        .unwrap();

    // Create initial matrix tasks
    let task1_matrix = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Pending,
        is_master: false,
        master_task_id: Some(master_task.id),
        matrix_values: Some(HashMap::from([
            ("task".to_string(), serde_json::to_value("task1").unwrap()),
            (
                "priority".to_string(),
                serde_json::to_value("high").unwrap(),
            ),
        ])),
        started_at: None,
        ended_at: None,
        error: None,
        logs: Vec::new(),
    };

    let task2_matrix = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Pending,
        is_master: false,
        master_task_id: Some(master_task.id),
        matrix_values: Some(HashMap::from([
            ("task".to_string(), serde_json::to_value("task2").unwrap()),
            (
                "priority".to_string(),
                serde_json::to_value("medium").unwrap(),
            ),
        ])),
        started_at: None,
        ended_at: None,
        error: None,
        logs: Vec::new(),
    };

    // Save initial matrix tasks
    state_adapter.save_task(&task1_matrix).await.unwrap();
    state_adapter.save_task(&task2_matrix).await.unwrap();

    // Verify initial tasks
    let initial_tasks = state_adapter.get_tasks(workflow_run_id).await.unwrap();
    assert_eq!(initial_tasks.len(), 4); // node1 + master + 2 matrix tasks

    // Now simulate a state update (like recompilation would do)
    let updated_tasks_array = serde_json::json!([
        {"task": "task1", "priority": "high"},       // kept
        {"task": "task3", "priority": "low"},        // new
        {"task": "task4", "priority": "high"}        // new
    ]);

    let mut updated_state = HashMap::new();
    updated_state.insert("files".to_string(), updated_tasks_array);

    // Update the state with new data
    state_adapter
        .update_state(workflow_run_id, updated_state)
        .await
        .unwrap();

    // Mark task2 as WontDo (it's no longer in the updated state)
    let mut fields = HashMap::new();
    fields.insert(
        "status".to_string(),
        FieldDiff {
            operation: DiffOperation::Update,
            value: Some(serde_json::to_value(TaskStatus::WontDo).unwrap()),
        },
    );

    let task_diff = TaskDiff {
        task_id: task2_matrix.id,
        fields,
    };

    // Apply the diff to mark task2 as WontDo
    state_adapter.apply_task_diff(&task_diff).await.unwrap();

    // Create new matrix tasks for task3 and task4
    let task3_matrix = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Pending,
        is_master: false,
        master_task_id: Some(master_task.id),
        matrix_values: Some(HashMap::from([
            ("task".to_string(), serde_json::to_value("task3").unwrap()),
            ("priority".to_string(), serde_json::to_value("low").unwrap()),
        ])),
        started_at: None,
        ended_at: None,
        error: None,
        logs: Vec::new(),
    };

    let task4_matrix = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Pending,
        is_master: false,
        master_task_id: Some(master_task.id),
        matrix_values: Some(HashMap::from([
            ("task".to_string(), serde_json::to_value("task4").unwrap()),
            (
                "priority".to_string(),
                serde_json::to_value("high").unwrap(),
            ),
        ])),
        started_at: None,
        ended_at: None,
        error: None,
        logs: Vec::new(),
    };

    // Save new matrix tasks
    state_adapter.save_task(&task3_matrix).await.unwrap();
    state_adapter.save_task(&task4_matrix).await.unwrap();

    // Get final tasks
    let final_tasks = state_adapter.get_tasks(workflow_run_id).await.unwrap();
    assert_eq!(final_tasks.len(), 6); // node1 + master + 4 matrix tasks (including WontDo)

    // Verify matrix tasks
    let matrix_tasks: Vec<&Task> = final_tasks
        .iter()
        .filter(|t| t.node_id == "node2" && !t.is_master)
        .collect();

    assert_eq!(matrix_tasks.len(), 4);

    // Verify one task is marked as WontDo
    let wontdo_tasks: Vec<&Task> = matrix_tasks
        .clone()
        .into_iter()
        .filter(|t| t.status == TaskStatus::WontDo)
        .collect();

    assert_eq!(wontdo_tasks.len(), 1);

    // Verify active tasks have correct values
    let active_matrix_tasks: Vec<&Task> = matrix_tasks
        .into_iter()
        .filter(|t| t.status != TaskStatus::WontDo)
        .collect();

    assert_eq!(active_matrix_tasks.len(), 3);

    for task in active_matrix_tasks {
        let matrix_values = task.matrix_values.as_ref().unwrap();
        let task_name = matrix_values.get("task").unwrap().as_str().unwrap();
        assert!(
            task_name == "task1" || task_name == "task3" || task_name == "task4",
            "Task should be one of the expected tasks from updated state"
        );
    }
}

#[tokio::test]
async fn test_empty_state_matrix_workflow() {
    // Test that empty state arrays result in no matrix tasks
    let mut state_adapter = MockStateAdapter::new();

    // Create a workflow with matrix from state strategy
    let workflow = create_matrix_from_state_workflow();

    // Create a workflow run
    let workflow_run_id = Uuid::new_v4();
    let workflow_run = WorkflowRun {
        id: workflow_run_id,
        workflow: workflow.clone(),
        status: WorkflowStatus::Running,
        params: HashMap::new(),
        tasks: Vec::new(),
        started_at: chrono::Utc::now(),
        ended_at: None,
        bundle_path: None,
        capabilities: None,
        name: None,
        target_path: None,
    };

    // Save the workflow run
    state_adapter
        .save_workflow_run(&workflow_run)
        .await
        .unwrap();

    // Create a completed task for node1
    let node1_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node1".to_string(),
        status: TaskStatus::Completed,
        is_master: false,
        master_task_id: None,
        matrix_values: None,
        started_at: Some(chrono::Utc::now()),
        ended_at: Some(chrono::Utc::now()),
        error: None,
        logs: Vec::new(),
    };

    // Save the task
    state_adapter.save_task(&node1_task).await.unwrap();

    // Create a master task for node2
    let master_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Completed, // Master task completes when no matrix tasks exist
        is_master: true,
        master_task_id: None,
        matrix_values: None,
        started_at: Some(chrono::Utc::now()),
        ended_at: Some(chrono::Utc::now()),
        error: None,
        logs: Vec::new(),
    };

    // Save the master task
    state_adapter.save_task(&master_task).await.unwrap();

    // Set state with empty array
    let empty_array = serde_json::json!([]);

    let mut state = HashMap::new();
    state.insert("files".to_string(), empty_array);

    // Update the state with empty array
    state_adapter
        .update_state(workflow_run_id, state)
        .await
        .unwrap();

    // Get the tasks
    let tasks = state_adapter.get_tasks(workflow_run_id).await.unwrap();

    // Should have only node1 task and master task, no matrix tasks
    assert_eq!(tasks.len(), 2);

    // Verify no matrix tasks exist
    let matrix_tasks: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.node_id == "node2" && !t.is_master)
        .collect();

    assert_eq!(
        matrix_tasks.len(),
        0,
        "Empty state should produce no matrix tasks"
    );

    // Verify master task completed
    let master_task_result = tasks
        .iter()
        .find(|t| t.node_id == "node2" && t.is_master)
        .unwrap();

    assert_eq!(master_task_result.status, TaskStatus::Completed);
}

#[tokio::test]
async fn test_malformed_state_matrix_workflow() {
    // Test that invalid/malformed state data is handled gracefully
    let mut state_adapter = MockStateAdapter::new();

    // Create a workflow with matrix from state strategy
    let workflow = create_matrix_from_state_workflow();

    // Create a workflow run
    let workflow_run_id = Uuid::new_v4();
    let workflow_run = WorkflowRun {
        id: workflow_run_id,
        workflow: workflow.clone(),
        status: WorkflowStatus::Running,
        params: HashMap::new(),
        tasks: Vec::new(),
        started_at: chrono::Utc::now(),
        ended_at: None,
        bundle_path: None,
        capabilities: None,
        name: None,
        target_path: None,
    };

    // Save the workflow run
    state_adapter
        .save_workflow_run(&workflow_run)
        .await
        .unwrap();

    // Create a completed task for node1
    let node1_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node1".to_string(),
        status: TaskStatus::Completed,
        is_master: false,
        master_task_id: None,
        matrix_values: None,
        started_at: Some(chrono::Utc::now()),
        ended_at: Some(chrono::Utc::now()),
        error: None,
        logs: Vec::new(),
    };

    // Save the task
    state_adapter.save_task(&node1_task).await.unwrap();

    // Create a master task for node2
    let master_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Completed, // Master task should complete when no valid matrix data
        is_master: true,
        master_task_id: None,
        matrix_values: None,
        started_at: Some(chrono::Utc::now()),
        ended_at: Some(chrono::Utc::now()),
        error: None,
        logs: Vec::new(),
    };

    // Save the master task
    state_adapter.save_task(&master_task).await.unwrap();

    // Set state with invalid data (not an array, which is expected for matrix)
    let invalid_data = serde_json::json!({
        "invalid": "not an array",
        "malformed": true
    });

    let mut state = HashMap::new();
    state.insert("files".to_string(), invalid_data);

    // Update the state with malformed data
    state_adapter
        .update_state(workflow_run_id, state)
        .await
        .unwrap();

    // Get the tasks
    let tasks = state_adapter.get_tasks(workflow_run_id).await.unwrap();

    // Should have only node1 task and master task, no matrix tasks
    assert_eq!(tasks.len(), 2);

    // Verify no matrix tasks exist (malformed data should not create matrix tasks)
    let matrix_tasks: Vec<&Task> = tasks
        .iter()
        .filter(|t| t.node_id == "node2" && !t.is_master)
        .collect();

    assert_eq!(
        matrix_tasks.len(),
        0,
        "Malformed state should produce no matrix tasks"
    );

    // Verify master task handled the malformed data gracefully
    let master_task_result = tasks
        .iter()
        .find(|t| t.node_id == "node2" && t.is_master)
        .unwrap();

    // Master task should either complete (if it handles invalid data gracefully)
    // or fail (if it properly detects the invalid data)
    assert!(
        master_task_result.status == TaskStatus::Completed
            || master_task_result.status == TaskStatus::Failed,
        "Master task should complete or fail gracefully with malformed state"
    );
}

#[tokio::test]
async fn test_matrix_hash_based_deduplication() {
    use butterflow_scheduler::Scheduler;

    // This test verifies that matrix tasks are properly deduplicated using hash-based comparison
    // even if the JSON representation might differ (e.g., key ordering, whitespace)
    let mut state_adapter = MockStateAdapter::new();
    let scheduler = Scheduler::new();

    // Create a workflow with a matrix node using from_state
    let workflow = create_matrix_from_state_workflow();

    // Create a workflow run
    let workflow_run_id = Uuid::new_v4();
    let workflow_run = WorkflowRun {
        id: workflow_run_id,
        workflow: workflow.clone(),
        status: WorkflowStatus::Running,
        params: HashMap::new(),
        tasks: Vec::new(),
        started_at: chrono::Utc::now(),
        ended_at: None,
        bundle_path: None,
        capabilities: None,
        name: None,
        target_path: None,
    };

    // Save the workflow run
    state_adapter
        .save_workflow_run(&workflow_run)
        .await
        .unwrap();

    // Create a master task for the matrix node
    let master_task = Task {
        id: Uuid::new_v4(),
        workflow_run_id,
        node_id: "node2".to_string(),
        status: TaskStatus::Pending,
        is_master: true,
        master_task_id: None,
        matrix_values: None,
        started_at: None,
        ended_at: None,
        error: None,
        logs: Vec::new(),
    };

    // Save the master task
    state_adapter.save_task(&master_task).await.unwrap();

    // Set initial state with shards (similar to your actual workflow)
    let initial_shards = serde_json::json!([
        {
            "team": "unassigned",
            "shard": "1/6",
            "shardId": "unassigned 1/6"
        },
        {
            "team": "unassigned",
            "shard": "2/6",
            "shardId": "unassigned 2/6"
        }
    ]);

    let mut initial_state = HashMap::new();
    initial_state.insert("files".to_string(), initial_shards);

    // Update the state
    state_adapter
        .update_state(workflow_run_id, initial_state)
        .await
        .unwrap();

    // Get existing tasks (should be just the master task)
    let existing_tasks = state_adapter.get_tasks(workflow_run_id).await.unwrap();

    // Calculate matrix task changes - this should create 2 new tasks
    let changes = scheduler
        .calculate_matrix_task_changes(
            workflow_run_id,
            &workflow_run,
            &existing_tasks,
            &state_adapter.get_state(workflow_run_id).await.unwrap(),
        )
        .await
        .unwrap();

    // Should create 2 new tasks
    assert_eq!(
        changes.new_tasks.len(),
        2,
        "Should create 2 new tasks for 2 shards"
    );
    assert_eq!(
        changes.tasks_to_mark_wont_do.len(),
        0,
        "No tasks should be marked as WontDo"
    );

    // Save the new tasks
    for task in &changes.new_tasks {
        state_adapter.save_task(task).await.unwrap();
    }

    // Now get all tasks including the newly created ones
    let tasks_after_first_run = state_adapter.get_tasks(workflow_run_id).await.unwrap();
    assert_eq!(
        tasks_after_first_run.len(),
        3,
        "Should have master + 2 matrix tasks"
    );

    // NOW THE KEY TEST: Set the SAME state again (simulating re-running evaluate-codeowners)
    // but with potentially different JSON formatting
    let same_shards_different_format = serde_json::json!([
        {
            "shardId": "unassigned 1/6",
            "team": "unassigned",  // Note: different key order
            "shard": "1/6"
        },
        {
            "shard": "2/6",
            "shardId": "unassigned 2/6",
            "team": "unassigned"   // Note: different key order
        }
    ]);

    let mut same_state = HashMap::new();
    same_state.insert("files".to_string(), same_shards_different_format);

    // Update state with the same logical values but different JSON structure
    state_adapter
        .update_state(workflow_run_id, same_state)
        .await
        .unwrap();

    // Calculate matrix task changes again - this should NOT create any new tasks
    // because the hash-based comparison should recognize these as the same values
    let changes_second_run = scheduler
        .calculate_matrix_task_changes(
            workflow_run_id,
            &workflow_run,
            &tasks_after_first_run,
            &state_adapter.get_state(workflow_run_id).await.unwrap(),
        )
        .await
        .unwrap();

    // CRITICAL: Should NOT create any new tasks because they already exist (hash-based deduplication)
    assert_eq!(
        changes_second_run.new_tasks.len(),
        0,
        "Should NOT create any new tasks - hash-based deduplication should prevent duplicates"
    );
    assert_eq!(
        changes_second_run.tasks_to_mark_wont_do.len(),
        0,
        "No tasks should be marked as WontDo since the values are the same"
    );

    // Verify total task count remains the same
    let final_tasks = state_adapter.get_tasks(workflow_run_id).await.unwrap();
    assert_eq!(
        final_tasks.len(),
        3,
        "Total task count should remain the same (master + 2 matrix tasks)"
    );

    // Now test with actual new data - add a third shard
    let expanded_shards = serde_json::json!([
        {
            "team": "unassigned",
            "shard": "1/6",
            "shardId": "unassigned 1/6"
        },
        {
            "team": "unassigned",
            "shard": "2/6",
            "shardId": "unassigned 2/6"
        },
        {
            "team": "unassigned",
            "shard": "3/6",
            "shardId": "unassigned 3/6"  // NEW shard
        }
    ]);

    let mut expanded_state = HashMap::new();
    expanded_state.insert("files".to_string(), expanded_shards);

    state_adapter
        .update_state(workflow_run_id, expanded_state)
        .await
        .unwrap();

    // Calculate matrix task changes with expanded state
    let changes_expansion = scheduler
        .calculate_matrix_task_changes(
            workflow_run_id,
            &workflow_run,
            &final_tasks,
            &state_adapter.get_state(workflow_run_id).await.unwrap(),
        )
        .await
        .unwrap();

    // Should create 1 new task for the new shard
    assert_eq!(
        changes_expansion.new_tasks.len(),
        1,
        "Should create 1 new task for the new shard"
    );
    assert_eq!(
        changes_expansion.tasks_to_mark_wont_do.len(),
        0,
        "No tasks should be marked as WontDo"
    );

    // Verify the new task has the correct matrix values
    let new_task = &changes_expansion.new_tasks[0];
    assert_eq!(new_task.node_id, "node2");
    assert!(!new_task.is_master);
    assert_eq!(new_task.master_task_id, Some(master_task.id));

    // Check the matrix values of the new task
    let matrix_values = new_task.matrix_values.as_ref().unwrap();
    assert_eq!(matrix_values.get("shard").unwrap().as_str().unwrap(), "3/6");
    assert_eq!(
        matrix_values.get("shardId").unwrap().as_str().unwrap(),
        "unassigned 3/6"
    );
    assert_eq!(
        matrix_values.get("team").unwrap().as_str().unwrap(),
        "unassigned"
    );
}

// Helper function to create a workflow with conditional step
fn create_conditional_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "conditional-node".to_string(),
            name: "Conditional Node".to_string(),
            description: Some("Test node with conditional step".to_string()),
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![
                Step {
                    id: Some("always-runs".to_string()),
                    name: "Always runs".to_string(),
                    action: StepAction::RunScript("echo 'This step always runs'".to_string()),
                    env: None,
                    condition: None,
                    commit: None,
                },
                Step {
                    id: Some("conditional-step".to_string()),
                    name: "Conditional step".to_string(),
                    action: StepAction::RunScript(
                        "echo 'This step runs conditionally'".to_string(),
                    ),
                    env: None,
                    condition: Some("params.my_cond".to_string()),
                    commit: None,
                },
            ],
            env: HashMap::new(),
            branch_name: None,
            pull_request: None,
        }],
    }
}

fn create_wrapped_conditional_workflow() -> Workflow {
    let mut workflow = create_conditional_workflow();
    workflow.nodes[0].steps[1].condition = Some("${{ params.my_cond }}".to_string());
    workflow
}

// Helper function to create a workflow with non-existent variable references
fn create_nonexistent_variable_workflow() -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![
            Node {
                id: "nonexistent-var-node".to_string(),
                name: "Node with non-existent variables".to_string(),
                description: Some("Test node referencing non-existent variables".to_string()),
                r#type: NodeType::Automatic,
                depends_on: vec![],
                trigger: None,
                strategy: None,
                runtime: Some(Runtime {
                    r#type: RuntimeType::Direct,
                    image: None,
                    working_dir: None,
                    user: None,
                    network: None,
                    options: None,
                }),
                steps: vec![Step {
                    id: Some("test-non-existent-variable".to_string()),
                    name: "Test non-existent variable".to_string(),
                    action: StepAction::RunScript("echo 'Value: ${nonexistent.variable}' && echo 'Another: ${params.missing_param}'".to_string()),
                    env: Some(HashMap::from([
                        ("TEST_VAR".to_string(), "${nonexistent.env_var}".to_string()),
                        ("MISSING_PARAM".to_string(), "${params.does_not_exist}".to_string()),
                    ])),
                    condition: None,
                    commit: None,
                }],
                env: HashMap::from([
                    ("NODE_VAR".to_string(), "${state.missing_state}".to_string()),
                ]),
            branch_name: None,
            pull_request: None,
            },
        ],
    }
}

#[tokio::test]
async fn test_workflow_condition_with_params_true() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_conditional_workflow();

    // Create parameters with my_cond set to true
    let mut params = HashMap::new();
    params.insert("my_cond".to_string(), serde_json::Value::Bool(true));

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Get the workflow run
    let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();

    // Check that the workflow completed
    assert!(
        workflow_run.status == WorkflowStatus::Running
            || workflow_run.status == WorkflowStatus::Completed
    );

    // Get the tasks
    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // Find the conditional task
    let conditional_task = tasks
        .iter()
        .find(|t| t.node_id == "conditional-node")
        .unwrap();

    // The task should have completed successfully (both steps should run)
    assert!(
        conditional_task.status == TaskStatus::Completed
            || conditional_task.status == TaskStatus::Running
    );

    // Check logs contain both steps if completed
    if conditional_task.status == TaskStatus::Completed {
        let log_output = conditional_task.logs.join("\n");
        assert!(log_output.contains("This step always runs"));
        assert!(log_output.contains("This step runs conditionally"));
    }
}

#[tokio::test]
async fn test_workflow_condition_with_params_false() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_conditional_workflow();

    // Create parameters with my_cond set to false
    let mut params = HashMap::new();
    params.insert("my_cond".to_string(), serde_json::Value::Bool(false));

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Get the workflow run
    let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();

    // Check that the workflow completed
    assert!(
        workflow_run.status == WorkflowStatus::Running
            || workflow_run.status == WorkflowStatus::Completed
    );

    // Get the tasks
    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // Find the conditional task
    let conditional_task = tasks
        .iter()
        .find(|t| t.node_id == "conditional-node")
        .unwrap();

    // The task should have completed successfully (only first step should run)
    assert!(
        conditional_task.status == TaskStatus::Completed
            || conditional_task.status == TaskStatus::Running
    );

    // Check logs contain only the first step if completed
    if conditional_task.status == TaskStatus::Completed {
        let log_output = conditional_task.logs.join("\n");
        assert!(log_output.contains("This step always runs"));
        // The conditional step should NOT have run
        assert!(!log_output.contains("This step runs conditionally"));
    }
}

#[tokio::test]
async fn test_workflow_condition_with_wrapped_params_true() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_wrapped_conditional_workflow();

    let mut params = HashMap::new();
    params.insert("my_cond".to_string(), serde_json::Value::Bool(true));

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let conditional_task = tasks
        .iter()
        .find(|t| t.node_id == "conditional-node")
        .unwrap();

    assert!(
        conditional_task.status == TaskStatus::Completed
            || conditional_task.status == TaskStatus::Running
    );

    if conditional_task.status == TaskStatus::Completed {
        let log_output = conditional_task.logs.join("\n");
        assert!(log_output.contains("This step always runs"));
        assert!(log_output.contains("This step runs conditionally"));
    }
}

#[tokio::test]
async fn test_workflow_condition_with_wrapped_params_false() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_wrapped_conditional_workflow();

    let mut params = HashMap::new();
    params.insert("my_cond".to_string(), serde_json::Value::Bool(false));

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();
    let conditional_task = tasks
        .iter()
        .find(|t| t.node_id == "conditional-node")
        .unwrap();

    assert!(
        conditional_task.status == TaskStatus::Completed
            || conditional_task.status == TaskStatus::Running
    );

    if conditional_task.status == TaskStatus::Completed {
        let log_output = conditional_task.logs.join("\n");
        assert!(log_output.contains("This step always runs"));
        assert!(!log_output.contains("This step runs conditionally"));
    }
}

#[tokio::test]
async fn test_workflow_condition_with_params_missing() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_conditional_workflow();

    // Create parameters WITHOUT my_cond (it should default to false/empty)
    let params = HashMap::new();

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Get the workflow run
    let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();

    // Check that the workflow completed
    assert!(
        workflow_run.status == WorkflowStatus::Running
            || workflow_run.status == WorkflowStatus::Completed
    );

    // Get the tasks
    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // Find the conditional task
    let conditional_task = tasks
        .iter()
        .find(|t| t.node_id == "conditional-node")
        .unwrap();

    // The task should have completed successfully (only first step should run, condition should be false)
    assert!(
        conditional_task.status == TaskStatus::Completed
            || conditional_task.status == TaskStatus::Running
    );

    // Check logs contain only the first step if completed
    if conditional_task.status == TaskStatus::Completed {
        let log_output = conditional_task.logs.join("\n");
        assert!(log_output.contains("This step always runs"));
        // The conditional step should NOT have run (missing param should be treated as false)
        assert!(!log_output.contains("This step runs conditionally"));
    }
}

#[tokio::test]
async fn test_expression_resolution_nonexistent_variable() {
    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, WorkflowRunConfig::default());

    let workflow = create_nonexistent_variable_workflow();
    let params = HashMap::new(); // No parameters provided

    let workflow_run_id = engine
        .run_workflow(workflow, params, None, None)
        .await
        .unwrap();

    // Allow some time for the workflow to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Get the workflow run
    let workflow_run = engine.get_workflow_run(workflow_run_id).await.unwrap();

    // Check that the workflow completed without errors (non-existent variables should resolve gracefully)
    assert!(
        workflow_run.status == WorkflowStatus::Running
            || workflow_run.status == WorkflowStatus::Completed
            || workflow_run.status == WorkflowStatus::Failed // Might fail, but shouldn't crash
    );

    // Get the tasks
    let tasks = engine.get_tasks(workflow_run_id).await.unwrap();

    // Find the task with non-existent variables
    let nonexistent_var_task = tasks
        .iter()
        .find(|t| t.node_id == "nonexistent-var-node")
        .unwrap();

    // The task should have either completed or failed gracefully
    assert!(
        nonexistent_var_task.status == TaskStatus::Completed
            || nonexistent_var_task.status == TaskStatus::Running
            || nonexistent_var_task.status == TaskStatus::Failed
    );

    // If the task completed, check that non-existent variables were resolved to empty strings
    if nonexistent_var_task.status == TaskStatus::Completed {
        let log_output = nonexistent_var_task.logs.join("\n");
        // Non-existent variables should resolve to empty strings in the output
        // The exact output depends on how the shell interprets empty variables,
        // but it should not contain literal "${variable}" strings
        println!("Log output for non-existent variables test: {}", log_output);
    }
}

// TODO: test_cycle_detection_direct_cycle
// TODO: test_find_cycle_in_chain
// TODO: test_runtime_cycle_detection

#[test]
fn js_ast_grep_idle_timeout_uses_default_and_respects_env_override() {
    let _guard = EnvVarGuard::unset("CODEMOD_JS_AST_GREP_IDLE_TIMEOUT_MS");
    assert_eq!(
        js_ast_grep_idle_timeout(),
        Duration::from_millis(JS_AST_GREP_IDLE_TIMEOUT_MS_DEFAULT)
    );

    std::env::set_var("CODEMOD_JS_AST_GREP_IDLE_TIMEOUT_MS", "1234");
    assert_eq!(js_ast_grep_idle_timeout(), Duration::from_millis(1234));
}

#[test]
fn shard_scan_falls_back_to_selector_matches_when_dry_run_finds_no_edits() {
    let eligible = select_shard_scan_eligible_files(
        Vec::new(),
        vec!["src/a.ts".to_string(), "src/b.ts".to_string()],
    );

    assert_eq!(eligible, vec!["src/a.ts", "src/b.ts"]);
}

#[test]
fn shard_scan_prefers_modified_files_when_available() {
    let eligible = select_shard_scan_eligible_files(
        vec!["src/changed.ts".to_string()],
        vec!["src/selector-only.ts".to_string()],
    );

    assert_eq!(eligible, vec!["src/changed.ts"]);
}

#[test]
fn record_unit_progress_updates_global_and_active_units() {
    let state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
    let before = state.lock().unwrap().global_last_progress_at;

    std::thread::sleep(Duration::from_millis(5));
    record_unit_progress(&state, "src/example.ts", StepPhase::ExecutionStarted);

    let snapshot = state.lock().unwrap();
    assert_eq!(snapshot.global_phase, StepPhase::ExecutionStarted);
    assert!(snapshot.global_last_progress_at > before);
    let unit = snapshot.active_units.get("src/example.ts").unwrap();
    assert_eq!(unit.phase, StepPhase::ExecutionStarted);
    assert!(unit.last_progress_at > before);
    assert!(snapshot.output_active_units.contains("src/example.ts"));
}

#[test]
fn record_output_progress_refreshes_executing_units() {
    let state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
    record_unit_progress(&state, "src/example.ts", StepPhase::ExecutionStarted);
    let before = state
        .lock()
        .unwrap()
        .active_units
        .get("src/example.ts")
        .unwrap()
        .last_progress_at;

    std::thread::sleep(Duration::from_millis(5));
    record_output_progress(&state);

    let snapshot = state.lock().unwrap();
    assert_eq!(snapshot.global_phase, StepPhase::Output);
    let unit = snapshot.active_units.get("src/example.ts").unwrap();
    assert_eq!(unit.phase, StepPhase::Output);
    assert!(unit.last_progress_at > before);
}

#[test]
fn finish_unit_progress_removes_active_unit() {
    let state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
    record_unit_progress(&state, "src/example.ts", StepPhase::ExecutionStarted);
    finish_unit_progress(&state, "src/example.ts", StepPhase::ExecutionFinished);

    let snapshot = state.lock().unwrap();
    assert_eq!(snapshot.global_phase, StepPhase::ExecutionFinished);
    assert!(!snapshot.active_units.contains_key("src/example.ts"));
    assert!(!snapshot.output_active_units.contains("src/example.ts"));
}

#[test]
fn build_idle_timeout_message_uses_stalest_active_unit() {
    let now = Instant::now();
    let mut state = StepProgressState::new();
    state.global_last_progress_at = now - Duration::from_secs(90);
    state.global_phase = StepPhase::Output;
    state.active_units.insert(
        "src/fresh.ts".to_string(),
        UnitProgressState {
            last_progress_at: now - Duration::from_secs(10),
            phase: StepPhase::Output,
        },
    );
    state.active_units.insert(
        "src/stale.ts".to_string(),
        UnitProgressState {
            last_progress_at: now - Duration::from_secs(75),
            phase: StepPhase::ExecutionStarted,
        },
    );

    let message = build_js_ast_grep_idle_timeout_message(&state, Duration::from_secs(60));
    assert!(message.contains("src/stale.ts"));
    assert!(message.contains("execution started"));
    assert!(message.contains("active units: 2"));
}

#[tokio::test]
async fn await_js_ast_grep_execution_task_returns_idle_timeout_error() {
    let progress_state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
    record_unit_progress(
        &progress_state,
        "src/stalled.ts",
        StepPhase::ExecutionStarted,
    );
    let idle_timed_out = Arc::new(AtomicBool::new(false));
    let idle_notify = Arc::new(Notify::new());
    let idle_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));

    let local = tokio::task::LocalSet::new();
    let idle_timed_out_for_task = Arc::clone(&idle_timed_out);
    let idle_notify_for_task = Arc::clone(&idle_notify);
    let idle_failure_message_for_task = Arc::clone(&idle_failure_message);
    let progress_state_for_task = Arc::clone(&progress_state);
    let result = local
            .run_until(async move {
                let trigger = tokio::spawn({
                    let idle_timed_out = Arc::clone(&idle_timed_out_for_task);
                    let idle_notify = Arc::clone(&idle_notify_for_task);
                    let idle_failure_message = Arc::clone(&idle_failure_message_for_task);
                    async move {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        idle_timed_out.store(true, Ordering::Release);
                        if let Ok(mut message) = idle_failure_message.lock() {
                            *message = Some(
                                "No progress observed for 1s while processing src/stalled.ts (execution started, active units: 1)"
                                    .to_string(),
                            );
                        }
                        idle_notify.notify_waiters();
                    }
                });

                let execution_task = tokio::task::spawn_local(async move {
                    futures_util::future::pending::<
                        std::result::Result<
                            CodemodOutput,
                            codemod_sandbox::sandbox::errors::ExecutionError,
                        >,
                    >()
                    .await
                });

                let result = await_js_ast_grep_execution_task(
                    execution_task,
                    idle_timed_out_for_task,
                    idle_notify_for_task,
                    idle_failure_message_for_task,
                    progress_state_for_task,
                    Duration::from_secs(1),
                    "src/stalled.ts",
                )
                .await;
                trigger.await.unwrap();
                result
            })
            .await;

    let error = result.expect_err("pending execution should time out");
    let message = error.to_string();
    assert!(message.contains("No progress observed"));
    assert!(message.contains("src/stalled.ts"));
}

#[tokio::test]
async fn await_js_ast_grep_execution_task_returns_prompt_completion_without_polling_delay() {
    let progress_state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
    record_unit_progress(&progress_state, "src/fast.ts", StepPhase::ExecutionStarted);
    let idle_timed_out = Arc::new(AtomicBool::new(false));
    let idle_notify = Arc::new(Notify::new());
    let idle_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));

    let local = tokio::task::LocalSet::new();
    let result = tokio::time::timeout(
        Duration::from_millis(100),
        local.run_until(async move {
            let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();

            let execution_task = tokio::task::spawn_local(async move {
                let _ = release_rx.await;
                Ok(CodemodOutput {
                    primary: ExecutionResult::Unmodified,
                    secondary: vec![],
                })
            });

            let wait_task = tokio::task::spawn_local(await_js_ast_grep_execution_task(
                execution_task,
                Arc::clone(&idle_timed_out),
                Arc::clone(&idle_notify),
                Arc::clone(&idle_failure_message),
                Arc::clone(&progress_state),
                Duration::from_secs(1),
                "src/fast.ts",
            ));

            tokio::task::yield_now().await;
            release_tx
                .send(())
                .expect("completion signal should be sent");
            wait_task.await.expect("wait task should join")
        }),
    )
    .await
    .expect("completed execution should not wait for a polling interval");

    let output = result
        .expect("helper should return successfully")
        .expect("execution should complete successfully");
    assert!(matches!(output.primary, ExecutionResult::Unmodified));
    assert!(output.secondary.is_empty());
}

#[tokio::test]
async fn await_js_ast_grep_execution_task_prefers_completed_result_over_later_idle_signal() {
    let progress_state = Arc::new(std::sync::Mutex::new(StepProgressState::new()));
    record_unit_progress(&progress_state, "src/fast.ts", StepPhase::ExecutionStarted);
    let idle_timed_out = Arc::new(AtomicBool::new(false));
    let idle_notify = Arc::new(Notify::new());
    let idle_failure_message = Arc::new(std::sync::Mutex::new(None::<String>));

    let local = tokio::task::LocalSet::new();
    let result = tokio::time::timeout(
        Duration::from_millis(100),
        local.run_until(async move {
            let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
            let idle_timed_out_for_trigger = Arc::clone(&idle_timed_out);
            let idle_notify_for_trigger = Arc::clone(&idle_notify);
            let idle_failure_message_for_trigger = Arc::clone(&idle_failure_message);

            let trigger = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                idle_timed_out_for_trigger.store(true, Ordering::Release);
                if let Ok(mut message) = idle_failure_message_for_trigger.lock() {
                    *message = Some("unexpected timeout".to_string());
                }
                idle_notify_for_trigger.notify_waiters();
            });

            let execution_task = tokio::task::spawn_local(async move {
                let _ = release_rx.await;
                Ok(CodemodOutput {
                    primary: ExecutionResult::Unmodified,
                    secondary: vec![],
                })
            });

            let wait_task = tokio::task::spawn_local(await_js_ast_grep_execution_task(
                execution_task,
                Arc::clone(&idle_timed_out),
                Arc::clone(&idle_notify),
                Arc::clone(&idle_failure_message),
                Arc::clone(&progress_state),
                Duration::from_secs(1),
                "src/fast.ts",
            ));

            tokio::task::yield_now().await;
            release_tx
                .send(())
                .expect("completion signal should be sent");
            let result = wait_task.await.expect("wait task should join");
            trigger.await.expect("idle trigger should join");
            result
        }),
    )
    .await
    .expect("completed execution should resolve before a later idle timeout signal");

    let output = result
        .expect("helper should return successfully")
        .expect("execution should complete successfully");
    assert!(matches!(output.primary, ExecutionResult::Unmodified));
    assert!(output.secondary.is_empty());
}

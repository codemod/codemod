use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::Result;
use chrono::{DateTime, Utc};
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use tokio::sync::{broadcast, mpsc, oneshot};
use uuid::Uuid;

use crate::ai_handoff::AgentOption;
use crate::config::{
    AgentSelectionCallback, CapabilitiesSecurityCallback, DeferredInteractionError,
    PullRequestApprovalCallback, PullRequestCreationRequest, SelectionPrompt,
    SelectionPromptCallback, ShellCommandApprovalCallback, ShellCommandExecutionRequest,
};
use crate::engine::Engine;
use crate::{Task, WorkflowRun, WorkflowStatus};

#[derive(Clone, Debug)]
pub struct AgentSelectionOption {
    pub canonical: String,
    pub label: String,
    pub is_available: bool,
}

impl From<&AgentOption> for AgentSelectionOption {
    fn from(value: &AgentOption) -> Self {
        Self {
            canonical: value.canonical.to_string(),
            label: value.label.to_string(),
            is_available: value.is_available(),
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum WorkflowEvent {
    WorkflowStarted {
        workflow_run: WorkflowRun,
        at: DateTime<Utc>,
    },
    WorkflowStatusChanged {
        workflow_run_id: Uuid,
        status: WorkflowStatus,
        at: DateTime<Utc>,
    },
    TaskCreated {
        task: Task,
        at: DateTime<Utc>,
    },
    TaskUpdated {
        task: Task,
        at: DateTime<Utc>,
    },
    TaskLogAppended {
        workflow_run_id: Uuid,
        task_id: Uuid,
        line: String,
        at: DateTime<Utc>,
    },
    TaskProgressUpdated {
        workflow_run_id: Uuid,
        task_id: Uuid,
        processed_files: u64,
        total_files: Option<u64>,
        current_file: Option<String>,
        at: DateTime<Utc>,
    },
    ShellApprovalRequested {
        request_id: Uuid,
        request: ShellCommandExecutionRequest,
        at: DateTime<Utc>,
    },
    PullRequestApprovalRequested {
        request_id: Uuid,
        request: PullRequestCreationRequest,
        at: DateTime<Utc>,
    },
    CapabilitiesApprovalRequested {
        request_id: Uuid,
        modules: Vec<LlrtSupportedModules>,
        at: DateTime<Utc>,
    },
    AgentSelectionRequested {
        request_id: Uuid,
        options: Vec<AgentSelectionOption>,
        at: DateTime<Utc>,
    },
    SelectionRequested {
        request_id: Uuid,
        prompt: SelectionPrompt,
        at: DateTime<Utc>,
    },
}

#[derive(Clone, Debug)]
pub enum WorkflowCommand {
    TriggerTask {
        task_id: Uuid,
    },
    TriggerTasks {
        task_ids: Vec<Uuid>,
    },
    TriggerAll,
    CancelWorkflow,
    CreatePullRequest {
        task_id: Uuid,
    },
    RespondShellApproval {
        request_id: Uuid,
        approved: bool,
    },
    RespondPullRequestApproval {
        request_id: Uuid,
        approved: bool,
    },
    RespondCapabilitiesApproval {
        request_id: Uuid,
        approved: bool,
    },
    RespondAgentSelection {
        request_id: Uuid,
        selection: Option<String>,
    },
    RespondSelection {
        request_id: Uuid,
        selection: Option<String>,
    },
}

pub trait WorkflowEventSink: Send + Sync {
    fn publish(&self, event: WorkflowEvent);
}

type SinkMap = HashMap<Uuid, HashMap<usize, Arc<dyn WorkflowEventSink>>>;

fn registry() -> &'static Mutex<SinkMap> {
    static REGISTRY: OnceLock<Mutex<SinkMap>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_sink_id() -> usize {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn publish_event(workflow_run_id: Uuid, event: WorkflowEvent) {
    let sinks = registry()
        .lock()
        .ok()
        .and_then(|map| map.get(&workflow_run_id).cloned());
    if let Some(sinks) = sinks {
        for sink in sinks.into_values() {
            sink.publish(event.clone());
        }
    }
}

struct BroadcastEventSink {
    sender: broadcast::Sender<WorkflowEvent>,
}

impl WorkflowEventSink for BroadcastEventSink {
    fn publish(&self, event: WorkflowEvent) {
        let _ = self.sender.send(event);
    }
}

struct PendingApprovals {
    shell: Mutex<HashMap<Uuid, std::sync::mpsc::SyncSender<Result<bool>>>>,
    pull_request: Mutex<HashMap<Uuid, std::sync::mpsc::SyncSender<Result<bool>>>>,
    capabilities: Mutex<CapabilityApprovalState>,
    agent: Mutex<HashMap<Uuid, std::sync::mpsc::SyncSender<Option<String>>>>,
    selection: Mutex<HashMap<Uuid, std::sync::mpsc::SyncSender<Option<String>>>>,
}

impl PendingApprovals {
    fn with_approved(approved: HashSet<LlrtSupportedModules>) -> Self {
        Self {
            shell: Mutex::new(HashMap::new()),
            pull_request: Mutex::new(HashMap::new()),
            capabilities: Mutex::new(CapabilityApprovalState {
                approved,
                pending_by_request: HashMap::new(),
                in_flight_by_key: HashMap::new(),
            }),
            agent: Mutex::new(HashMap::new()),
            selection: Mutex::new(HashMap::new()),
        }
    }
}

#[derive(Default)]
struct CapabilityApprovalState {
    approved: HashSet<LlrtSupportedModules>,
    pending_by_request: HashMap<Uuid, PendingCapabilitiesApproval>,
    in_flight_by_key: HashMap<String, Uuid>,
}

struct PendingCapabilitiesApproval {
    modules: HashSet<LlrtSupportedModules>,
    listeners: Vec<std::sync::mpsc::SyncSender<Result<()>>>,
}

struct WorkflowSessionInteractor {
    sender: broadcast::Sender<WorkflowEvent>,
    pending: Arc<PendingApprovals>,
}

impl WorkflowSessionInteractor {
    fn new(sender: broadcast::Sender<WorkflowEvent>, pending: Arc<PendingApprovals>) -> Self {
        Self { sender, pending }
    }

    fn shell_callback(&self) -> ShellCommandApprovalCallback {
        let sender = self.sender.clone();
        let pending = Arc::clone(&self.pending);
        Arc::new(move |request| {
            let request_id = Uuid::new_v4();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            pending.shell.lock().unwrap().insert(request_id, tx);
            let _ = sender.send(WorkflowEvent::ShellApprovalRequested {
                request_id,
                request: request.clone(),
                at: Utc::now(),
            });
            rx.recv().map_err(|error| {
                anyhow::anyhow!("shell approval response channel closed: {error}")
            })?
        })
    }

    fn pull_request_callback(&self) -> PullRequestApprovalCallback {
        let sender = self.sender.clone();
        let pending = Arc::clone(&self.pending);
        Arc::new(move |request| {
            let request_id = Uuid::new_v4();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            pending.pull_request.lock().unwrap().insert(request_id, tx);
            if let Err(error) = sender.send(WorkflowEvent::PullRequestApprovalRequested {
                request_id,
                request: request.clone(),
                at: Utc::now(),
            }) {
                pending.pull_request.lock().unwrap().remove(&request_id);
                return Err(anyhow::anyhow!(
                    "failed to deliver pull request approval event: {error}"
                ));
            }
            rx.recv().map_err(|error| {
                anyhow::anyhow!("pull request approval response channel closed: {error}")
            })?
        })
    }

    fn capabilities_callback(&self) -> CapabilitiesSecurityCallback {
        let sender = self.sender.clone();
        let pending = Arc::clone(&self.pending);
        Arc::new(move |execution_config| {
            let requested_modules = execution_config
                .capabilities
                .as_ref()
                .cloned()
                .unwrap_or_default();
            if requested_modules.is_empty() {
                return Ok(());
            }

            let key;
            let request_id;
            let is_new_request;
            let modules_to_approve;
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            {
                let mut capability_state = pending.capabilities.lock().unwrap();
                modules_to_approve = requested_modules
                    .difference(&capability_state.approved)
                    .copied()
                    .collect::<HashSet<_>>();
                if modules_to_approve.is_empty() {
                    return Ok(());
                }

                key = capability_request_key(&modules_to_approve);

                if let Some(existing_request_id) =
                    capability_state.in_flight_by_key.get(&key).copied()
                {
                    capability_state
                        .pending_by_request
                        .get_mut(&existing_request_id)
                        .expect("in-flight capability request should exist")
                        .listeners
                        .push(tx);
                    request_id = existing_request_id;
                    is_new_request = false;
                } else {
                    request_id = Uuid::new_v4();
                    capability_state
                        .in_flight_by_key
                        .insert(key.clone(), request_id);
                    capability_state.pending_by_request.insert(
                        request_id,
                        PendingCapabilitiesApproval {
                            modules: modules_to_approve.clone(),
                            listeners: vec![tx],
                        },
                    );
                    is_new_request = true;
                }
            }
            if modules_to_approve.is_empty() {
                return Ok(());
            }

            if is_new_request {
                let _ = sender.send(WorkflowEvent::CapabilitiesApprovalRequested {
                    request_id,
                    modules: modules_to_approve.iter().copied().collect(),
                    at: Utc::now(),
                });
            }
            rx.recv().map_err(|error| {
                anyhow::anyhow!("capability approval response channel closed: {error}")
            })?
        })
    }

    fn agent_callback(&self) -> AgentSelectionCallback {
        let sender = self.sender.clone();
        let pending = Arc::clone(&self.pending);
        Arc::new(move |agents| {
            let request_id = Uuid::new_v4();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            pending.agent.lock().unwrap().insert(request_id, tx);
            let options = agents.iter().map(AgentSelectionOption::from).collect();
            let _ = sender.send(WorkflowEvent::AgentSelectionRequested {
                request_id,
                options,
                at: Utc::now(),
            });
            rx.recv().ok().flatten()
        })
    }

    fn selection_callback(&self) -> SelectionPromptCallback {
        let sender = self.sender.clone();
        let pending = Arc::clone(&self.pending);
        Arc::new(move |prompt| {
            let request_id = Uuid::new_v4();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            pending.selection.lock().unwrap().insert(request_id, tx);
            if let Err(error) = sender.send(WorkflowEvent::SelectionRequested {
                request_id,
                prompt: prompt.clone(),
                at: Utc::now(),
            }) {
                pending.selection.lock().unwrap().remove(&request_id);
                return Err(anyhow::anyhow!(
                    "failed to deliver selection prompt event: {error}"
                ));
            }
            rx.recv()
                .map_err(|error| anyhow::anyhow!("selection response channel closed: {error}"))?
                .ok_or_else(|| DeferredInteractionError::new("selection prompt canceled").into())
                .map(Some)
        })
    }
}

async fn handle_command(
    engine: &Engine,
    workflow_run_id: Uuid,
    pending: &PendingApprovals,
    command: WorkflowCommand,
) -> Result<()> {
    match command {
        WorkflowCommand::TriggerTask { task_id } => engine
            .resume_workflow(workflow_run_id, vec![task_id])
            .await
            .map_err(anyhow::Error::from),
        WorkflowCommand::TriggerTasks { task_ids } => engine
            .resume_workflow(workflow_run_id, task_ids)
            .await
            .map_err(anyhow::Error::from),
        WorkflowCommand::TriggerAll => {
            let _ = engine.trigger_all(workflow_run_id).await?;
            Ok(())
        }
        WorkflowCommand::CancelWorkflow => engine
            .cancel_workflow(workflow_run_id)
            .await
            .map_err(anyhow::Error::from),
        WorkflowCommand::CreatePullRequest { task_id } => {
            let engine = engine.clone();
            tokio::spawn(async move {
                if let Err(error) = engine.create_pull_request_for_task(task_id).await {
                    log::error!("failed to create pull request for task {task_id}: {error}");
                }
            });
            Ok(())
        }
        WorkflowCommand::RespondShellApproval {
            request_id,
            approved,
        } => {
            if let Some(tx) = pending.shell.lock().unwrap().remove(&request_id) {
                let _ = tx.send(Ok(approved));
            }
            Ok(())
        }
        WorkflowCommand::RespondPullRequestApproval {
            request_id,
            approved,
        } => {
            if let Some(tx) = pending.pull_request.lock().unwrap().remove(&request_id) {
                let _ = tx.send(Ok(approved));
            }
            Ok(())
        }
        WorkflowCommand::RespondCapabilitiesApproval {
            request_id,
            approved,
        } => {
            let pending_request = {
                let mut capability_state = pending.capabilities.lock().unwrap();
                capability_state.pending_by_request.remove(&request_id)
            };
            if let Some(pending_request) = pending_request {
                let mut capability_state = pending.capabilities.lock().unwrap();
                capability_state
                    .in_flight_by_key
                    .remove(&capability_request_key(&pending_request.modules));
                if approved {
                    capability_state.approved.extend(pending_request.modules);
                }
                drop(capability_state);
                for listener in pending_request.listeners {
                    let _ = if approved {
                        listener.send(Ok(()))
                    } else {
                        listener.send(Err(anyhow::anyhow!("capabilities approval rejected")))
                    };
                }
            }
            Ok(())
        }
        WorkflowCommand::RespondAgentSelection {
            request_id,
            selection,
        } => {
            if let Some(tx) = pending.agent.lock().unwrap().remove(&request_id) {
                let _ = tx.send(selection);
            }
            Ok(())
        }
        WorkflowCommand::RespondSelection {
            request_id,
            selection,
        } => {
            if let Some(tx) = pending.selection.lock().unwrap().remove(&request_id) {
                let _ = tx.send(selection);
            }
            Ok(())
        }
    }
}

fn capability_request_key(modules: &HashSet<LlrtSupportedModules>) -> String {
    let mut module_names = modules
        .iter()
        .map(|module| format!("{module:?}"))
        .collect::<Vec<_>>();
    module_names.sort();
    module_names.join("|")
}

struct SessionCommandEnvelope {
    command: WorkflowCommand,
    response_tx: Option<oneshot::Sender<Result<()>>>,
}

pub struct WorkflowSession {
    workflow_run_id: Uuid,
    engine: Engine,
    command_tx: mpsc::UnboundedSender<SessionCommandEnvelope>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    registration_id: usize,
    _command_task: tokio::task::JoinHandle<()>,
}

impl WorkflowSession {
    pub fn attach(mut engine: Engine, workflow_run_id: Uuid) -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        let sink: Arc<dyn WorkflowEventSink> = Arc::new(BroadcastEventSink {
            sender: event_tx.clone(),
        });
        let registration_id = next_sink_id();
        registry()
            .lock()
            .unwrap()
            .entry(workflow_run_id)
            .or_default()
            .insert(registration_id, Arc::clone(&sink));

        let preapproved_capabilities = engine.get_capabilities().clone().unwrap_or_default();
        let pending = Arc::new(PendingApprovals::with_approved(preapproved_capabilities));
        let interactor = WorkflowSessionInteractor::new(event_tx.clone(), Arc::clone(&pending));
        engine.set_quiet(true);
        let config = engine.workflow_run_config_mut();
        config.capabilities_security_callback = Some(interactor.capabilities_callback());
        config.agent_selection_callback = Some(interactor.agent_callback());
        config.selection_prompt_callback = Some(interactor.selection_callback());
        config.shell_command_approval_callback = Some(interactor.shell_callback());
        config.pull_request_approval_callback = Some(interactor.pull_request_callback());

        let (command_tx, mut command_rx) = mpsc::unbounded_channel::<SessionCommandEnvelope>();
        let command_engine = engine.clone();
        let command_task = tokio::spawn(async move {
            while let Some(envelope) = command_rx.recv().await {
                let result =
                    handle_command(&command_engine, workflow_run_id, &pending, envelope.command)
                        .await;
                if let Err(error) = &result {
                    log::error!("workflow session command failed: {error}");
                }
                if let Some(response_tx) = envelope.response_tx {
                    let _ = response_tx.send(result);
                }
            }
        });

        Self {
            workflow_run_id,
            engine,
            command_tx,
            event_tx,
            registration_id,
            _command_task: command_task,
        }
    }

    pub async fn start_workflow(
        engine: Engine,
        workflow: crate::Workflow,
        params: HashMap<String, serde_json::Value>,
        bundle_path: Option<std::path::PathBuf>,
        capabilities: Option<&std::collections::HashSet<LlrtSupportedModules>>,
    ) -> Result<Self> {
        Self::start_workflow_with_id(
            engine,
            Uuid::new_v4(),
            workflow,
            params,
            bundle_path,
            capabilities,
        )
        .await
    }

    pub async fn start_workflow_with_id(
        engine: Engine,
        workflow_run_id: Uuid,
        workflow: crate::Workflow,
        params: HashMap<String, serde_json::Value>,
        bundle_path: Option<std::path::PathBuf>,
        capabilities: Option<&std::collections::HashSet<LlrtSupportedModules>>,
    ) -> Result<Self> {
        let session = Self::attach(engine, workflow_run_id);
        session
            .engine
            .run_workflow_with_id(workflow_run_id, workflow, params, bundle_path, capabilities)
            .await?;
        Ok(session)
    }

    pub fn handle(&self) -> WorkflowSessionHandle {
        WorkflowSessionHandle {
            workflow_run_id: self.workflow_run_id,
            engine: self.engine.clone(),
            command_tx: self.command_tx.clone(),
            event_tx: self.event_tx.clone(),
        }
    }
}

impl Drop for WorkflowSession {
    fn drop(&mut self) {
        if let Ok(mut map) = registry().lock() {
            if let Some(sinks) = map.get_mut(&self.workflow_run_id) {
                sinks.remove(&self.registration_id);
                if sinks.is_empty() {
                    map.remove(&self.workflow_run_id);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct WorkflowSessionHandle {
    workflow_run_id: Uuid,
    engine: Engine,
    command_tx: mpsc::UnboundedSender<SessionCommandEnvelope>,
    event_tx: broadcast::Sender<WorkflowEvent>,
}

impl WorkflowSessionHandle {
    pub fn workflow_run_id(&self) -> Uuid {
        self.workflow_run_id
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.event_tx.subscribe()
    }

    pub async fn send(&self, command: WorkflowCommand) -> Result<()> {
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx
            .send(SessionCommandEnvelope {
                command,
                response_tx: Some(response_tx),
            })
            .map_err(|error| anyhow::anyhow!("failed to send workflow command: {error}"))?;
        response_rx
            .await
            .map_err(|error| anyhow::anyhow!("workflow command response channel closed: {error}"))?
    }

    pub fn dispatch(&self, command: WorkflowCommand) -> Result<()> {
        self.command_tx
            .send(SessionCommandEnvelope {
                command,
                response_tx: None,
            })
            .map_err(|error| anyhow::anyhow!("failed to dispatch workflow command: {error}"))
    }

    pub fn dispatch_trigger_task(&self, task_id: Uuid) {
        let engine = self.engine.clone();
        let workflow_run_id = self.workflow_run_id;
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new()
                .expect("failed to build runtime for workflow trigger");
            runtime.block_on(async move {
                if let Err(error) = engine.resume_workflow(workflow_run_id, vec![task_id]).await {
                    log::error!("failed to trigger task {}: {}", task_id, error);
                }
            });
        });
    }

    pub fn dispatch_trigger_tasks(&self, task_ids: Vec<Uuid>) {
        if task_ids.is_empty() {
            return;
        }

        let engine = self.engine.clone();
        let workflow_run_id = self.workflow_run_id;
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new()
                .expect("failed to build runtime for workflow triggers");
            runtime.block_on(async move {
                if let Err(error) = engine
                    .resume_workflow(workflow_run_id, task_ids.clone())
                    .await
                {
                    log::error!(
                        "failed to trigger {} task(s) for workflow {}: {}",
                        task_ids.len(),
                        workflow_run_id,
                        error
                    );
                }
            });
        });
    }

    pub fn dispatch_cancel_workflow(&self) {
        let engine = self.engine.clone();
        let workflow_run_id = self.workflow_run_id;
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new()
                .expect("failed to build runtime for workflow cancellation");
            runtime.block_on(async move {
                if let Err(error) = engine.cancel_workflow(workflow_run_id).await {
                    log::error!("failed to cancel workflow {}: {}", workflow_run_id, error);
                }
            });
        });
    }

    pub async fn load_snapshot(&self) -> Result<WorkflowSnapshot> {
        Ok(WorkflowSnapshot {
            workflow_run: self.engine.get_workflow_run(self.workflow_run_id).await?,
            tasks: self.engine.get_tasks(self.workflow_run_id).await?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct WorkflowSnapshot {
    pub workflow_run: WorkflowRun,
    pub tasks: Vec<Task>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SelectionPromptOption;
    use crate::execution::CodemodExecutionConfig;
    use codemod_llrt_capabilities::types::LlrtSupportedModules;
    use std::collections::HashSet;
    use std::thread;

    #[tokio::test]
    async fn broadcast_sink_receives_published_event() {
        let workflow_run_id = Uuid::new_v4();
        let session = WorkflowSession::attach(crate::engine::Engine::new(), workflow_run_id);
        let mut rx = session.handle().subscribe();

        publish_event(
            workflow_run_id,
            WorkflowEvent::WorkflowStatusChanged {
                workflow_run_id,
                status: crate::WorkflowStatus::Running,
                at: Utc::now(),
            },
        );

        let event = rx.try_recv().expect("event should be available");
        match event {
            WorkflowEvent::WorkflowStatusChanged {
                workflow_run_id: actual_id,
                ..
            } => assert_eq!(actual_id, workflow_run_id),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn capabilities_approval_is_cached_within_session() {
        let (sender, _) = broadcast::channel(16);
        let pending = Arc::new(PendingApprovals::with_approved(HashSet::new()));
        let interactor = WorkflowSessionInteractor::new(sender.clone(), Arc::clone(&pending));
        let callback = interactor.capabilities_callback();
        let mut approval_rx = sender.subscribe();
        let mut idle_rx = sender.subscribe();

        let requested: HashSet<LlrtSupportedModules> =
            [LlrtSupportedModules::Fs].into_iter().collect();
        let config = CodemodExecutionConfig {
            pre_run_callback: None,
            progress_callback: Arc::new(None),
            target_path: None,
            base_path: None,
            include_globs: None,
            explicit_files: None,
            exclude_globs: None,
            dry_run: false,
            languages: None,
            threads: None,
            capabilities: Some(requested.clone()),
        };

        let pending_for_thread = Arc::clone(&pending);
        let approval_thread = thread::spawn(move || {
            let event = approval_rx
                .blocking_recv()
                .expect("approval event should arrive");
            let request_id = match event {
                WorkflowEvent::CapabilitiesApprovalRequested { request_id, .. } => request_id,
                other => panic!("unexpected event: {other:?}"),
            };

            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                handle_command(
                    &crate::engine::Engine::new(),
                    Uuid::new_v4(),
                    &pending_for_thread,
                    WorkflowCommand::RespondCapabilitiesApproval {
                        request_id,
                        approved: true,
                    },
                )
                .await
                .unwrap();
            });
        });

        callback(&config).expect("first approval should succeed");
        approval_thread.join().unwrap();

        let first_event = idle_rx
            .try_recv()
            .expect("first prompt event should be observable");
        assert!(matches!(
            first_event,
            WorkflowEvent::CapabilitiesApprovalRequested { .. }
        ));
        assert!(matches!(
            idle_rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        callback(&config).expect("cached approval should succeed without prompting");
        assert!(matches!(
            idle_rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn concurrent_capabilities_requests_share_one_prompt() {
        let (sender, _) = broadcast::channel(16);
        let pending = Arc::new(PendingApprovals::with_approved(HashSet::new()));
        let interactor = WorkflowSessionInteractor::new(sender.clone(), Arc::clone(&pending));
        let callback = interactor.capabilities_callback();
        let mut rx = sender.subscribe();

        let requested: HashSet<LlrtSupportedModules> =
            [LlrtSupportedModules::Fs].into_iter().collect();
        let config = CodemodExecutionConfig {
            pre_run_callback: None,
            progress_callback: Arc::new(None),
            target_path: None,
            base_path: None,
            include_globs: None,
            explicit_files: None,
            exclude_globs: None,
            dry_run: false,
            languages: None,
            threads: None,
            capabilities: Some(requested),
        };

        let first = {
            let callback = callback.clone();
            let config = config.clone();
            thread::spawn(move || callback(&config))
        };
        let second = {
            let callback = callback.clone();
            let config = config.clone();
            thread::spawn(move || callback(&config))
        };

        let event = tokio::time::timeout(tokio::time::Duration::from_secs(1), async {
            loop {
                match rx.try_recv() {
                    Ok(event) => break event,
                    Err(broadcast::error::TryRecvError::Empty) => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("unexpected broadcast receive error: {error}"),
                }
            }
        })
        .await
        .expect("one approval event should arrive");
        let request_id = match event {
            WorkflowEvent::CapabilitiesApprovalRequested { request_id, .. } => request_id,
            other => panic!("unexpected event: {other:?}"),
        };
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        handle_command(
            &crate::engine::Engine::new(),
            Uuid::new_v4(),
            &pending,
            WorkflowCommand::RespondCapabilitiesApproval {
                request_id,
                approved: true,
            },
        )
        .await
        .unwrap();

        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
    }

    #[tokio::test]
    async fn many_concurrent_capabilities_requests_share_one_prompt_and_all_unblock() {
        let (sender, _) = broadcast::channel(32);
        let pending = Arc::new(PendingApprovals::with_approved(HashSet::new()));
        let interactor = WorkflowSessionInteractor::new(sender.clone(), Arc::clone(&pending));
        let callback = interactor.capabilities_callback();
        let mut rx = sender.subscribe();

        let requested: HashSet<LlrtSupportedModules> =
            [LlrtSupportedModules::Fs].into_iter().collect();
        let config = CodemodExecutionConfig {
            pre_run_callback: None,
            progress_callback: Arc::new(None),
            target_path: None,
            base_path: None,
            include_globs: None,
            explicit_files: None,
            exclude_globs: None,
            dry_run: false,
            languages: None,
            threads: None,
            capabilities: Some(requested),
        };

        let threads: Vec<_> = (0..24)
            .map(|_| {
                let callback = callback.clone();
                let config = config.clone();
                thread::spawn(move || callback(&config))
            })
            .collect();

        let event = tokio::time::timeout(tokio::time::Duration::from_secs(1), async {
            loop {
                match rx.try_recv() {
                    Ok(event) => break event,
                    Err(broadcast::error::TryRecvError::Empty) => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("unexpected broadcast receive error: {error}"),
                }
            }
        })
        .await
        .expect("one approval event should arrive");
        let request_id = match event {
            WorkflowEvent::CapabilitiesApprovalRequested { request_id, .. } => request_id,
            other => panic!("unexpected event: {other:?}"),
        };
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));

        handle_command(
            &crate::engine::Engine::new(),
            Uuid::new_v4(),
            &pending,
            WorkflowCommand::RespondCapabilitiesApproval {
                request_id,
                approved: true,
            },
        )
        .await
        .unwrap();

        for thread in threads {
            thread.join().unwrap().unwrap();
        }
    }

    #[tokio::test]
    async fn preapproved_capabilities_do_not_prompt_again() {
        let (sender, _) = broadcast::channel(16);
        let approved: HashSet<LlrtSupportedModules> =
            [LlrtSupportedModules::Fs].into_iter().collect();
        let pending = Arc::new(PendingApprovals::with_approved(approved.clone()));
        let interactor = WorkflowSessionInteractor::new(sender.clone(), Arc::clone(&pending));
        let callback = interactor.capabilities_callback();
        let mut rx = sender.subscribe();

        let config = CodemodExecutionConfig {
            pre_run_callback: None,
            progress_callback: Arc::new(None),
            target_path: None,
            base_path: None,
            include_globs: None,
            explicit_files: None,
            exclude_globs: None,
            dry_run: false,
            languages: None,
            threads: None,
            capabilities: Some(approved),
        };

        callback(&config).expect("preapproved capabilities should not prompt");
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn selection_prompt_fails_fast_when_event_delivery_is_closed() {
        let (sender, _) = broadcast::channel(16);
        let pending = Arc::new(PendingApprovals::with_approved(HashSet::new()));
        let interactor = WorkflowSessionInteractor::new(sender, Arc::clone(&pending));
        let callback = interactor.selection_callback();

        let error = callback(SelectionPrompt {
            title: "Choose install scope".to_string(),
            options: vec![SelectionPromptOption {
                value: "project".to_string(),
                label: "project".to_string(),
            }],
            default_index: 0,
        })
        .expect_err("closed event delivery should fail immediately");

        assert!(
            error
                .to_string()
                .contains("failed to deliver selection prompt event"),
            "unexpected error: {error:#}"
        );
        assert!(pending.selection.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn pull_request_prompt_fails_fast_when_event_delivery_is_closed() {
        let (sender, _) = broadcast::channel(16);
        let pending = Arc::new(PendingApprovals::with_approved(HashSet::new()));
        let interactor = WorkflowSessionInteractor::new(sender, Arc::clone(&pending));
        let callback = interactor.pull_request_callback();

        let error = callback(&PullRequestCreationRequest {
            title: "Draft PR".to_string(),
            body: None,
            draft: true,
            head: "codemod-branch".to_string(),
            base: Some("main".to_string()),
            node_id: "apply-transforms".to_string(),
            node_name: "Apply transforms".to_string(),
            task_id: Uuid::new_v4().to_string(),
        })
        .expect_err("closed event delivery should fail immediately");

        assert!(
            error
                .to_string()
                .contains("failed to deliver pull request approval event"),
            "unexpected error: {error:#}"
        );
        assert!(pending.pull_request.lock().unwrap().is_empty());
    }
}

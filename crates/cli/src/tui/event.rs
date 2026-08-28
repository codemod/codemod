use butterflow_core::workflow_runtime::WorkflowEvent;
use butterflow_core::workflow_runtime::WorkflowSnapshot;

#[derive(Clone, Debug)]
pub enum AppEvent {
    Workflow(WorkflowEvent),
    Snapshot(WorkflowSnapshot),
}

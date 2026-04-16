use butterflow_core::workflow_runtime::WorkflowEvent;
use butterflow_core::workflow_runtime::WorkflowSnapshot;

use crate::tui::app::StatusBanner;

#[derive(Clone, Debug)]
pub enum AppEvent {
    Workflow(WorkflowEvent),
    Snapshot(WorkflowSnapshot),
    Banner(StatusBanner),
}

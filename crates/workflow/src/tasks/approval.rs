//! `approval(message)`: wait for a person before continuing.

use serde::Deserialize;
use serde_json::json;

use super::{TaskContext, TaskError, TaskOutcome, TaskResult};
use crate::interaction::{Approval, Decision};

#[derive(Deserialize)]
pub(crate) struct Request {
    message: String,
}

/// Approved, the step succeeds. Rejected, the run ends as rejected, and
/// steps running alongside stop; what earlier steps changed stays. Deferred,
/// the run waits to be resumed.
pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    match task.approve(&Approval::Manual {
        message: request.message.clone(),
    }) {
        Decision::Approved => Ok(TaskOutcome::new(json!({ "approved": true }))),
        Decision::Denied => {
            task.run.halt.reject(request.message.clone());
            Err(TaskError::Rejected(request.message))
        }
        Decision::Deferred => Err(TaskError::AwaitingApproval(request.message)),
    }
}

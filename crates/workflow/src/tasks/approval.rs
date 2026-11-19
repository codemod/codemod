//! `approval(message)`: wait for a person before continuing.

use serde::Deserialize;
use serde_json::json;

use super::{TaskContext, TaskError, TaskOutcome, TaskResult};
use crate::interaction::{Approval, Decision};

#[derive(Deserialize)]
pub(crate) struct Request {
    message: String,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    match task.run.interaction.approve(&Approval::Manual {
        message: request.message.clone(),
    }) {
        Decision::Approved => Ok(TaskOutcome::new(json!({ "approved": true }))),
        Decision::Denied => Err(TaskError::Failed(format!(
            "not approved: {}",
            request.message
        ))),
        Decision::Deferred => Err(TaskError::AwaitingApproval(request.message)),
    }
}

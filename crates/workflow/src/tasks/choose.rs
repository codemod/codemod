//! `choose(message, options, { default })`: ask a person to pick one option.
//!
//! Interactive runs ask; a run nobody can answer takes `default`, else the
//! first option. The answer is the step's result (a string), recorded like
//! any other, so a resume does not ask again.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;

use super::{TaskContext, TaskError, TaskOutcome, TaskResult};
use crate::interaction::Ask;

#[derive(Deserialize)]
pub(crate) struct Request {
    message: String,
    options: Vec<String>,
    #[serde(default)]
    default: Option<String>,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    if request.options.is_empty() {
        return Err(TaskError::Failed(
            "choose: give at least one option".to_string(),
        ));
    }
    if let Some(default) = &request.default {
        if !request.options.contains(default) {
            return Err(TaskError::Failed(format!(
                "choose: the default '{default}' is not one of the options"
            )));
        }
    }
    let step = task.at().step;
    let ask = Ask {
        id: format!("choice:{step}"),
        step: Some(step),
        stop: Some(Arc::clone(&task.cancel)),
    };
    let answer = task
        .run
        .interaction
        .choose(&ask, &request.message, &request.options)
        .filter(|answer| request.options.contains(answer));
    let picked = match answer {
        Some(answer) => answer,
        None => {
            let picked = request
                .default
                .clone()
                .unwrap_or_else(|| request.options[0].clone());
            task.log(format!(
                "{}: nobody answered, using '{picked}'",
                request.message
            ));
            picked
        }
    };
    Ok(TaskOutcome::new(json!(picked)))
}

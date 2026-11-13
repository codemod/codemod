//! `shell(command, { env, cwd })`.
//!
//! Keeps the YAML engine's environment contract: `CODEMOD_STATE` (a file
//! holding the state as JSON), `CODEMOD_STATE_<KEY>`, `CODEMOD_PATH`, and
//! `STATE_OUTPUTS`, a file the command can write `key=value` or
//! `key@=value` (append) lines to, which become state changes.

use std::collections::BTreeMap;
use std::sync::Arc;

use butterflow_runners::direct_runner::DirectRunner;
use butterflow_runners::Runner;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{block_on, child_env, TaskContext, TaskError, TaskOutcome, TaskResult};
use crate::interaction::{Approval, Decision};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Request {
    command: String,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Directory to run in, relative to the target.
    cwd: Option<String>,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    let run = task.run;
    if run.dry_run {
        task.log("skipped: shell commands do not run in a dry run");
        return Ok(TaskOutcome::new(json!({ "skipped": true })));
    }
    match run.interaction.approve(&Approval::ShellCommand {
        command: request.command.clone(),
    }) {
        Decision::Approved => {}
        Decision::Denied => {
            return Err(TaskError::Failed(
                "the shell command was not approved".into(),
            ))
        }
        Decision::Deferred => {
            return Err(TaskError::AwaitingApproval(format!(
                "approve running: {}",
                request.command
            )))
        }
    }

    let mut env = child_env(task);
    for (key, value) in &task.state {
        env.insert(
            format!("CODEMOD_STATE_{}", key.to_uppercase()),
            value.to_string(),
        );
    }
    env.extend(request.env.clone());

    let scratch = tempfile::Builder::new()
        .prefix("codemod-step-")
        .tempdir()
        .map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let state_input = scratch.path().join("state.json");
    let state_outputs = scratch.path().join("state-outputs");
    std::fs::write(&state_input, Value::Object(task.state.clone()).to_string())
        .and_then(|_| std::fs::write(&state_outputs, ""))
        .map_err(|e| format!("cannot prepare the step's state files: {e}"))?;
    env.insert(
        "CODEMOD_STATE".into(),
        state_input.to_string_lossy().into_owned(),
    );
    env.insert(
        "STATE_OUTPUTS".into(),
        state_outputs.to_string_lossy().into_owned(),
    );

    let command = match &request.cwd {
        Some(cwd) if !request.command.starts_with("#!") => {
            let dir = run.target.join(cwd);
            format!(
                "cd {} && {}",
                shell_quote(&dir.to_string_lossy()),
                request.command
            )
        }
        _ => request.command.clone(),
    };

    let events = Arc::clone(&run.events);
    let position = task.position;
    let on_line: butterflow_runners::OutputCallback = Arc::new(move |line: String| {
        // The runner tags each line with its stream; stdout needs no tag in step logs.
        let line = line
            .strip_prefix("[stdio] ")
            .or_else(|| line.strip_prefix("[stdout] "))
            .map(str::to_string)
            .unwrap_or(line);
        events.emit(crate::events::Event::StepLog { position, line });
    });
    let runner = DirectRunner::with_quiet(true).with_cancel(Arc::clone(&run.cancel));
    let output =
        block_on(runner.run_command(&command, &env, Some(on_line))).map_err(|e| match e {
            butterflow_models::Error::Canceled => TaskError::Canceled,
            other => TaskError::Failed(other.to_string()),
        })?;

    let outputs = std::fs::read_to_string(&state_outputs).unwrap_or_default();
    let state = parse_state_outputs(&outputs, &task.state);
    Ok(TaskOutcome::new(json!({ "output": output })).with_state(state))
}

/// Apply `key=value` (set) and `key@=value` (append) lines over `current`.
/// Values are JSON, falling back to plain strings.
pub(crate) fn parse_state_outputs(
    text: &str,
    current: &serde_json::Map<String, Value>,
) -> BTreeMap<String, Value> {
    let mut changes: BTreeMap<String, Value> = BTreeMap::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (key, append, raw) = if let Some((key, value)) = line.split_once("@=") {
            (key, true, value)
        } else if let Some((key, value)) = line.split_once('=') {
            (key, false, value)
        } else {
            continue;
        };
        let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
        if append {
            let existing = changes
                .get(key)
                .cloned()
                .or_else(|| current.get(key).cloned());
            let mut list = match existing {
                Some(Value::Array(items)) => items,
                Some(Value::Null) | None => Vec::new(),
                Some(other) => vec![other],
            };
            list.push(value);
            changes.insert(key.to_string(), Value::Array(list));
        } else {
            changes.insert(key.to_string(), value);
        }
    }
    changes
}

fn shell_quote(text: &str) -> String {
    if cfg!(windows) {
        format!("\"{text}\"")
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_outputs_set_and_append() {
        let mut current = serde_json::Map::new();
        current.insert("files".into(), json!(["a"]));
        let changes = parse_state_outputs(
            "count=3\nname=plain text\nfiles@=\"b\"\nfiles@=\"c\"\nmalformed\n",
            &current,
        );
        assert_eq!(changes["count"], json!(3));
        assert_eq!(changes["name"], json!("plain text"));
        assert_eq!(changes["files"], json!(["a", "b", "c"]));
        assert_eq!(changes.len(), 3);
    }
}

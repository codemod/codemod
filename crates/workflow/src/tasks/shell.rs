//! `shell(command, { env, cwd })`.
//!
//! The command gets `CODEMOD_PATH` and `CODEMOD_OUTPUT`, a file it can write
//! `key=value` or `key@=value` (append) lines to. They come back on the
//! step's result as `values`, so a later step can be given them. The same
//! file is also `STATE_OUTPUTS`, its name in workflow.yaml.

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
    env.extend(request.env.clone());

    let scratch = tempfile::Builder::new()
        .prefix("codemod-step-")
        .tempdir()
        .map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let values_file = scratch.path().join("output");
    std::fs::write(&values_file, "")
        .map_err(|e| format!("cannot prepare the step's output file: {e}"))?;
    let values_path = values_file.to_string_lossy().into_owned();
    // `STATE_OUTPUTS` is the YAML engine's name for the same file, so
    // scripts written for workflow.yaml keep working, on every platform.
    if !request.env.contains_key("STATE_OUTPUTS") {
        env.insert("STATE_OUTPUTS".into(), values_path.clone());
    }
    env.insert("CODEMOD_OUTPUT".into(), values_path);

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
    let runner = DirectRunner::with_quiet(true).with_cancel(Arc::clone(&task.cancel));
    let output =
        block_on(runner.run_command(&command, &env, Some(on_line))).map_err(|e| match e {
            butterflow_models::Error::Canceled => TaskError::Canceled,
            other => TaskError::Failed(other.to_string()),
        })?;

    let written = std::fs::read_to_string(&values_file).unwrap_or_default();
    let values = parse_values(&written);
    let mut result = json!({ "output": output });
    if !values.is_empty() {
        result["values"] = json!(values);
    }
    Ok(TaskOutcome::new(result))
}

/// Read `key=value` (set) and `key@=value` (append to a list) lines. A line
/// splits at its first `=`, so a value may contain `=` and `@=`. Values are
/// JSON, falling back to plain strings.
pub(crate) fn parse_values(text: &str) -> BTreeMap<String, Value> {
    let mut changes: BTreeMap<String, Value> = BTreeMap::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Some((key, raw)) = line.split_once('=') else {
            continue;
        };
        let (key, append) = match key.strip_suffix('@') {
            Some(key) => (key, true),
            None => (key, false),
        };
        if key.is_empty() {
            continue;
        }
        let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()));
        if append {
            let existing = changes.get(key).cloned();
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
    fn output_lines_set_and_append_values() {
        let values =
            parse_values("count=3\nname=plain text\nfiles@=\"b\"\nfiles@=\"c\"\nmalformed\n");
        assert_eq!(
            Value::Object(values.into_iter().collect()),
            json!({ "count": 3, "name": "plain text", "files": ["b", "c"] })
        );
    }

    #[test]
    fn an_output_line_splits_at_its_first_equals_sign() {
        let values = parse_values("note=a@=b\nquery=x=1\nlist@=1=2\n=5\n");
        assert_eq!(
            Value::Object(values.into_iter().collect()),
            json!({ "note": "a@=b", "query": "x=1", "list": ["1=2"] })
        );
    }
}

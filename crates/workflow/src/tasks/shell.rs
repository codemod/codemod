//! `shell(command, { env, cwd, allowFailure })`.
//!
//! The step's result is what the command printed and how it exited:
//! `{ stdout, stderr, exitCode }`. The workflow parses what it needs, such
//! as JSON on stdout's last line.

use std::collections::BTreeMap;
use std::sync::Arc;

use butterflow_runners::direct_runner::{DirectRunner, KEPT_OUTPUT};
use butterflow_runners::Runner;
use serde::Deserialize;
use serde_json::json;

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
    /// Return a failing command's result instead of failing the step.
    #[serde(default)]
    allow_failure: bool,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    let run = task.run;
    if run.dry_run {
        task.log("skipped: shell commands do not run in a dry run");
        return Ok(TaskOutcome::new(json!({ "skipped": true })));
    }
    match task.approve(&Approval::ShellCommand {
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

    let target_dir = if run.target.is_file() {
        run.target.parent().unwrap_or(&run.target)
    } else {
        &run.target
    };
    let cwd = target_dir.join(request.cwd.as_deref().unwrap_or(""));

    let events = Arc::clone(&run.events);
    let at = task.at();
    let on_line: butterflow_runners::OutputCallback = Arc::new(move |line: String| {
        // The runner tags each line with its stream; stdout needs no tag in step logs.
        let line = line
            .strip_prefix("[stdout] ")
            .map(str::to_string)
            .unwrap_or(line);
        events.emit(crate::events::Event::StepLog {
            at: at.clone(),
            line,
        });
    });
    let runner = DirectRunner::with_quiet(true)
        .with_cancel(Arc::clone(&task.cancel))
        .with_cwd(cwd);
    let output = block_on(runner.run_command(&request.command, &env, Some(on_line))).map_err(
        |e| match e {
            butterflow_models::Error::Canceled => TaskError::Canceled,
            other => TaskError::Failed(other.to_string()),
        },
    )?;

    if output.cut {
        task.log(format!(
            "the command printed more than {} MiB; the result keeps the end of it",
            KEPT_OUTPUT / (1024 * 1024)
        ));
    }
    if output.exit_code != 0 && !request.allow_failure {
        let printed = if output.stderr.trim().is_empty() {
            &output.stdout
        } else {
            &output.stderr
        };
        return Err(TaskError::Failed(format!(
            "the command exited with code {}{}",
            output.exit_code,
            last_lines(printed, 20)
        )));
    }
    Ok(TaskOutcome::new(json!({
        "stdout": output.stdout,
        "stderr": output.stderr,
        "exitCode": output.exit_code,
    })))
}

/// The last `count` lines of `text`, on lines of their own after a colon.
fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(
        ":\n{}",
        lines[lines.len().saturating_sub(count)..].join("\n")
    )
}

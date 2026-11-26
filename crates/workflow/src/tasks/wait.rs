//! `sleep(duration)` and `waitForEvent(name)`: steps that wait.
//!
//! Both count from when the step first began to wait (`TaskContext::since`),
//! which the orchestrator records, so a run that is interrupted and resumed
//! waits for what is left and not from the start.

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use super::{TaskContext, TaskError, TaskOutcome, TaskResult};

/// How often a waiting step looks for a cancel, the time, and its event.
const POLL: Duration = Duration::from_millis(100);

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Wait until `deadline` (milliseconds since the epoch), or until `done`
/// returns something. `Ok(None)` when the deadline passed.
fn wait_until<T>(
    task: &TaskContext<'_>,
    deadline: Option<i64>,
    mut done: impl FnMut() -> Result<Option<T>, TaskError>,
) -> Result<Option<T>, TaskError> {
    loop {
        if task.canceled() {
            return Err(TaskError::Canceled);
        }
        if let Some(found) = done()? {
            return Ok(Some(found));
        }
        let left = match deadline {
            Some(deadline) if deadline <= now() => return Ok(None),
            Some(deadline) => Duration::from_millis((deadline - now()).max(0) as u64),
            None => POLL,
        };
        std::thread::sleep(left.min(POLL));
    }
}

#[derive(Deserialize)]
pub(crate) struct Sleep {
    /// Milliseconds.
    duration: u64,
}

pub(crate) fn sleep(task: &TaskContext<'_>, request: Sleep) -> TaskResult {
    // A dry run shows what the workflow would change; it has nothing to
    // wait for.
    if !task.run.dry_run {
        let deadline = task.since.unwrap_or_else(now) + request.duration as i64;
        wait_until(task, Some(deadline), || Ok(None::<()>))?;
    }
    Ok(TaskOutcome::new(Value::Null))
}

#[derive(Deserialize)]
pub(crate) struct ForEvent {
    name: String,
    /// Milliseconds; without it the step waits until the event comes.
    #[serde(default)]
    timeout: Option<u64>,
}

pub(crate) fn for_event(task: &TaskContext<'_>, request: ForEvent) -> TaskResult {
    let run = task.run;
    if run.dry_run {
        return Err(TaskError::Failed(format!(
            "a dry run is not saved, so it cannot be sent the event '{}' it waits for here",
            request.name
        )));
    }
    // Events are sent to the run a person started, also for the codemods
    // nested in it.
    let run_id = run.run_id.split('~').next().unwrap_or(&run.run_id);
    let deadline = request
        .timeout
        .map(|timeout| task.since.unwrap_or_else(now) + timeout as i64);
    let event = wait_until(task, deadline, || {
        run.store
            .take_event(run_id, &request.name)
            .map_err(|e| TaskError::Failed(format!("cannot read the run's events: {e}")))
    })?;
    match event {
        Some(data) => Ok(TaskOutcome::new(data)),
        None => Err(TaskError::Failed(format!(
            "no '{}' event came within {}",
            request.name,
            describe(request.timeout.unwrap_or_default())
        ))),
    }
}

/// A duration in milliseconds, as a person would say it.
pub(crate) fn describe(ms: u64) -> String {
    match ms {
        ms if ms >= 3_600_000 && ms % 3_600_000 == 0 => format!("{}h", ms / 3_600_000),
        ms if ms >= 60_000 && ms % 60_000 == 0 => format!("{}m", ms / 60_000),
        ms if ms >= 1_000 && ms % 1_000 == 0 => format!("{}s", ms / 1_000),
        ms => format!("{ms}ms"),
    }
}

#[cfg(test)]
mod tests {
    use super::describe;

    #[test]
    fn durations_read_in_their_largest_whole_unit() {
        assert_eq!(describe(7_200_000), "2h");
        assert_eq!(describe(90_000), "90s");
        assert_eq!(describe(600_000), "10m");
        assert_eq!(describe(250), "250ms");
    }
}

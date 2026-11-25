use anyhow::{anyhow, Context, Result};
use butterflow_state::run_store::{RunStatus, StepStatus};
use clap::Args;
use codemod_workflow::send_event;

use crate::workflow_exec::store;

#[derive(Args, Debug)]
pub struct Command {
    /// The event's name, as the workflow's `waitForEvent(name)` has it
    event: String,

    /// Workflow run ID
    #[arg(short, long)]
    id: String,

    /// What the waiting step resolves to, as JSON (default: null)
    #[arg(long)]
    data: Option<String>,
}

/// Send an event to a workflow run. A step waiting for it continues; if no
/// step waits yet, the event is kept until one does. Events are for
/// `waitForEvent` steps: approvals are given with `workflow resume --approve`.
pub async fn handler(args: &Command) -> Result<()> {
    let data = match &args.data {
        Some(text) => serde_json::from_str(text).context("--data must be JSON")?,
        None => serde_json::Value::Null,
    };
    let store = store();
    let run = store
        .get_run(&args.id)?
        .ok_or_else(|| anyhow!("run '{}' not found", args.id))?;
    let steps = store.load_steps(&args.id)?;
    let waits_for_it = steps
        .iter()
        .any(|step| step.status == StepStatus::Waiting && waits_for_event(&step.key, &args.event));
    // An event does not give an approval: say how to, instead of keeping an
    // event nothing will take.
    if run.status == RunStatus::AwaitingApproval && !waits_for_it {
        let approve = run
            .question
            .as_deref()
            .map_or("--approve".to_string(), |question| {
                format!("--approve {question}")
            });
        return Err(anyhow!(
            "run {} waits for an approval{}, not for an event; give it with: codemod workflow resume --id {} {approve}",
            args.id,
            run.awaiting
                .as_deref()
                .map(|message| format!(" ({message})"))
                .unwrap_or_default(),
            args.id,
        ));
    }
    send_event(store.as_ref(), &args.id, &args.event, &data)?;
    if waits_for_it {
        println!("Sent '{}' to run {}.", args.event, args.id);
    } else {
        println!(
            "Sent '{}' to run {}; no step waits for it yet, so it is kept until one does.",
            args.event, args.id
        );
    }
    Ok(())
}

/// Whether a step's key (its canonical `[label, task]`) is a
/// `waitForEvent(name)` step.
fn waits_for_event(key: &str, name: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(key)
        .ok()
        .and_then(|key| {
            let task = key.get(1)?;
            Some(task.get("kind")? == "waitForEvent" && task.get("name")? == name)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::waits_for_event;

    #[test]
    fn a_wait_for_event_step_is_found_by_name() {
        let key = r#"[null,{"kind":"waitForEvent","name":"ci-finished"}]"#;
        assert!(waits_for_event(key, "ci-finished"));
        assert!(!waits_for_event(key, "approval"));
        assert!(!waits_for_event(
            r#"[null,{"kind":"approval","message":"x"}]"#,
            "approval"
        ));
        assert!(!waits_for_event("not json", "approval"));
    }
}

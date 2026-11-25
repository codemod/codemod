use anyhow::{anyhow, Result};
use butterflow_state::run_store::{RunRecord, StepRecord};
use clap::Args;
use serde_json::{json, Value};
use tabled::{Table, Tabled};

use crate::workflow_exec::{status_label, store};

/// How `workflow status` and `workflow list` print.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListFormat {
    #[default]
    Text,
    /// One JSON document on stdout
    Json,
}

#[derive(Args, Debug)]
pub struct Command {
    /// Workflow run ID
    #[arg(short, long)]
    id: String,

    /// Output format: "text" (default), or "json" with the steps' results and the run's output
    #[arg(long, value_enum, default_value_t = ListFormat::Text)]
    format: ListFormat,
}

/// A run as `--format json` prints it. `steps` is left out when `None`.
pub(crate) fn run_json(
    store: &dyn butterflow_state::run_store::RunStore,
    run: &RunRecord,
    steps: Option<&[StepRecord]>,
) -> Value {
    let mut value = json!({
        "id": run.id,
        "name": run.name,
        "workflow": run.workflow_path,
        "target": run.target,
        "params": run.params,
        "status": status_label(store, run),
        "startedAt": run.started_at.to_rfc3339(),
        "endedAt": run.ended_at.map(|ended| ended.to_rfc3339()),
        "durationMs": run.ended_at.map(|ended| (ended - run.started_at).num_milliseconds().max(0)),
        "awaiting": run.awaiting,
        "question": run.question,
        "error": run.error,
        "output": run.output,
        "exitCode": run.exit_code,
    });
    if let Some(steps) = steps {
        value["failedFiles"] = steps
            .iter()
            .map(StepRecord::failed_files)
            .sum::<usize>()
            .into();
        value["steps"] = steps
            .iter()
            .map(|step| {
                json!({
                    "position": step.position,
                    "label": step.label,
                    "kind": codemod_workflow::events::step_kind(&step.key),
                    "status": step.status,
                    "error": step.error,
                    "result": step.result,
                    "finishedAt": step.finished_at.and_then(chrono::DateTime::from_timestamp_millis).map(|at| at.to_rfc3339()),
                    "durationMs": step.duration_ms,
                    "attempts": step.attempts,
                    "files": step.files,
                })
            })
            .collect();
    }
    value
}

#[derive(Tabled)]
struct StepRow {
    #[tabled(rename = "#")]
    position: u64,
    #[tabled(rename = "Step")]
    label: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Error")]
    error: String,
}

/// Show a workflow run and its steps
pub async fn handler(args: &Command) -> Result<()> {
    let store = store();
    let run = store
        .get_run(&args.id)?
        .ok_or_else(|| anyhow!("run '{}' not found", args.id))?;
    let steps = store.load_steps(&args.id)?;
    if args.format == ListFormat::Json {
        println!("{}", run_json(store.as_ref(), &run, Some(&steps)));
        return Ok(());
    }

    println!("Run:       {}", run.id);
    if let Some(name) = &run.name {
        println!("Name:      {name}");
    }
    println!("Workflow:  {}", run.workflow_path.display());
    println!("Target:    {}", run.target.display());
    println!("Status:    {}", status_label(store.as_ref(), &run));
    println!("Started:   {}", run.started_at.format("%Y-%m-%d %H:%M:%S"));
    if let Some(ended) = run.ended_at {
        println!(
            "Ended:     {} ({})",
            ended.format("%Y-%m-%d %H:%M:%S"),
            humantime(ended - run.started_at)
        );
    }
    if let Some(awaiting) = &run.awaiting {
        println!("Waiting:   {awaiting}");
    }
    if let Some(question) = &run.question {
        println!(
            "Question:  {question} (codemod workflow resume --id {} --approve {question})",
            run.id
        );
    }
    if let Some(error) = &run.error {
        println!("Error:     {error}");
    }
    if let Some(code) = run.exit_code {
        println!("Exit code: {code}");
    }
    if let Some(output) = run.output.as_ref().filter(|output| !output.is_null()) {
        let json = output.to_string();
        if json.chars().count() > 200 {
            let cut: String = json.chars().take(200).collect();
            println!("Output:    {cut}… (all of it: --format json)");
        } else {
            println!("Output:    {json}");
        }
    }

    if steps.is_empty() {
        println!("\nNo steps recorded yet.");
        return Ok(());
    }
    let rows: Vec<StepRow> = steps
        .iter()
        .map(|step| StepRow {
            position: step.position,
            label: step.label.clone(),
            status: step.status.as_str().to_string(),
            error: step
                .error
                .as_deref()
                .map(|e| e.lines().next().unwrap_or_default().to_string())
                .unwrap_or_default(),
        })
        .collect();
    println!("\n{}", Table::new(rows));
    Ok(())
}

fn humantime(duration: chrono::TimeDelta) -> String {
    let seconds = duration.num_milliseconds() as f64 / 1000.0;
    if seconds < 60.0 {
        format!("{seconds:.1}s")
    } else {
        format!("{}m {}s", (seconds / 60.0) as u64, (seconds % 60.0) as u64)
    }
}

use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::Args;
use std::io::Write as _;

use crate::control::{Hello, HelloSettings, PROTOCOL};
use crate::workflow_exec::store;

/// How often a live run's journal is read for new events.
const POLL: Duration = Duration::from_millis(200);

#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttachFormat {
    /// One JSON event per line, as `--format jsonl` prints them
    #[default]
    Jsonl,
}

#[derive(Args, Debug)]
pub struct Command {
    /// Workflow run ID
    #[arg(short, long)]
    id: String,

    /// Output format: "jsonl"
    #[arg(long, value_enum, default_value_t = AttachFormat::Jsonl)]
    format: AttachFormat,

    /// Skip the events numbered below this (`seq`): the ones already seen
    #[arg(long, value_name = "SEQ")]
    from_seq: Option<u64>,
}

/// Print a run's events: a `hello` line, every event the run has had so far
/// (from the run's journal), then, while the run is executing in another
/// process, its events as they come. Ends when the run stops executing.
pub async fn handler(args: &Command) -> Result<()> {
    // Keep the attached run's sequence numbers and event stream intact.
    eprintln!(
        "{}",
        serde_json::json!({"event": "experimental", "api": "workflow attach"})
    );
    let store = store();
    let run = store
        .get_run(&args.id)?
        .ok_or_else(|| anyhow!("run '{}' not found", args.id))?;
    Hello {
        event: "hello",
        protocol: PROTOCOL,
        cli: crate::CLI_VERSION,
        run_id: run.id.clone(),
        mode: "attach",
        dry_run: false,
        workflow: run.workflow_path.clone(),
        target: run.target.clone(),
        settings: HelloSettings {
            capabilities: run.settings.capabilities.clone(),
            managed_git: run.settings.managed_git,
            agent: run.settings.agent.clone(),
            decision_model: None,
            approve_all: false,
            approved: Vec::new(),
        },
        commands: Vec::new(),
    }
    .print();
    let mut cursor = 0;
    loop {
        // Looked at before reading: whatever a run that just stopped wrote
        // is read on the way out.
        let live = store.is_held(&args.id)?;
        let (lines, next) = store.read_journal(&args.id, cursor)?;
        cursor = next;
        let mut stdout = std::io::stdout().lock();
        for line in lines {
            if let Some(from) = args.from_seq {
                let seq = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|event| event.get("seq")?.as_u64());
                if seq.is_some_and(|seq| seq < from) {
                    continue;
                }
            }
            writeln!(stdout, "{line}")?;
        }
        stdout.flush()?;
        drop(stdout);
        if !live {
            return Ok(());
        }
        tokio::time::sleep(POLL).await;
    }
}

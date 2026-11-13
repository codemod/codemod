//! Runs workflows on the code-first engine (`codemod-workflow`) and renders
//! its events. Every command that runs or resumes a workflow goes through
//! here.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use butterflow_core::config::InstallSkillExecutor;
use butterflow_core::registry::RegistryClient;
use butterflow_core::structured_log::OutputFormat;
use butterflow_state::run_store::{store_from_env, RunStore};
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_workflow::events::{Event, EventSink};
use codemod_workflow::interaction::{AgentChoice, Approval, Decision, Interaction, NonInteractive};
use codemod_workflow::{
    load_workflow, Ending, Engine, RunReport, RunStatus, Services, StartRequest, StepStatus,
    Summary,
};
use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use serde_json::Value;

/// Settings shared by starting and resuming a run.
pub struct ExecSettings {
    pub no_interactive: bool,
    pub format: OutputFormat,
    pub capabilities: HashSet<LlrtSupportedModules>,
    pub agent: Option<String>,
    pub registry: Option<RegistryClient>,
    pub install_skill: Option<Arc<dyn InstallSkillExecutor>>,
    /// Treat every approval as given.
    pub approve_all: bool,
    /// Let `branch`, `commit` and `pullRequest` steps touch git.
    pub managed_git: bool,
    pub skip_shard_steps: bool,
    pub nested_observer:
        Option<Arc<dyn butterflow_core::nested_codemod_run::NestedCodemodRunObserver>>,
}

pub struct ExecRequest {
    pub workflow: PathBuf,
    pub target: PathBuf,
    pub params: HashMap<String, Value>,
    pub name: Option<String>,
    pub dry_run: bool,
    pub settings: ExecSettings,
}

pub struct ExecOutcome {
    pub report: RunReport,
    pub summary: Arc<Summary>,
    pub duration: Duration,
}

impl ExecOutcome {
    pub fn files_modified(&self) -> usize {
        self.summary.stats.files_modified.load(Ordering::Relaxed)
    }

    pub fn files_unmodified(&self) -> usize {
        self.summary.stats.files_unmodified.load(Ordering::Relaxed)
    }

    pub fn files_with_errors(&self) -> usize {
        self.summary.stats.files_with_errors.load(Ordering::Relaxed)
    }

    /// `Ok` when the run succeeded or is waiting for an approval. Failures
    /// were already shown while the run progressed.
    pub fn result(&self) -> Result<()> {
        match &self.report.ending {
            Ending::Succeeded(_) | Ending::AwaitingApproval(_) | Ending::Stopped(_) => Ok(()),
            Ending::Failed(_) | Ending::Canceled => Err(WorkflowFailureAlreadyReported.into()),
        }
    }
}

/// A workflow failure the run's output already described.
#[derive(Debug)]
pub struct WorkflowFailureAlreadyReported;

impl std::fmt::Display for WorkflowFailureAlreadyReported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("workflow failed")
    }
}

impl std::error::Error for WorkflowFailureAlreadyReported {}

/// The run store selected by `CODEMOD_RUN_STORE`.
pub fn store() -> Result<Arc<dyn RunStore>> {
    Ok(Arc::from(
        store_from_env().context("cannot open the run store")?,
    ))
}

pub async fn start(request: ExecRequest) -> Result<ExecOutcome> {
    let workflow = load_workflow(&request.workflow).map_err(|e| anyhow!("{e}"))?;
    let format = request.settings.format;
    let (engine, renderer, summary) = engine(&request.settings)?;
    let started = Instant::now();
    let report = run_off_thread(engine, move |engine| {
        engine.start(StartRequest {
            workflow,
            target: request.target,
            params: Value::Object(request.params.into_iter().collect()),
            name: request.name,
            dry_run: request.dry_run,
            test: None,
        })
    })
    .await?;
    renderer.finish();
    let outcome = ExecOutcome {
        report,
        summary,
        duration: started.elapsed(),
    };
    print_ending(&outcome, format);
    Ok(outcome)
}

pub async fn resume(run_id: &str, settings: ExecSettings) -> Result<ExecOutcome> {
    let format = settings.format;
    let (engine, renderer, summary) = engine(&settings)?;
    let started = Instant::now();
    let run_id = run_id.to_string();
    let report = run_off_thread(engine, move |engine| engine.resume(&run_id)).await?;
    renderer.finish();
    let outcome = ExecOutcome {
        report,
        summary,
        duration: started.elapsed(),
    };
    print_ending(&outcome, format);
    Ok(outcome)
}

fn engine(settings: &ExecSettings) -> Result<(Engine, Arc<Renderer>, Arc<Summary>)> {
    let renderer = Arc::new(Renderer::new(settings.format));
    let interaction: Arc<dyn Interaction> = Arc::new(TerminalInteraction {
        interactive: !settings.no_interactive,
        approve_all: settings.approve_all,
        renderer: Arc::clone(&renderer),
        prompt_lock: Mutex::new(()),
    });
    let summary = Arc::new(Summary::default());
    let services = Services {
        capabilities: settings.capabilities.clone(),
        registry: settings.registry.clone(),
        install_skill: settings.install_skill.clone(),
        managed_git: settings.managed_git,
        agent: settings.agent.clone(),
        skip_shard_steps: settings.skip_shard_steps,
        nested_observer: settings.nested_observer.clone(),
        package_registry: Some(Arc::new(codemod_deps::HttpRegistry::new())),
        summary: Arc::clone(&summary),
    };
    let engine = Engine::new(store()?, renderer.clone(), interaction, services);
    Ok((engine, renderer, summary))
}

/// Run the engine on its own thread (its tasks start their own async
/// runtimes), stopping it on Ctrl-C.
async fn run_off_thread<F>(engine: Engine, run: F) -> Result<RunReport>
where
    F: FnOnce(&Engine) -> Result<RunReport, codemod_workflow::EngineError> + Send + 'static,
{
    let cancel = engine.cancel_flag();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(run(&engine));
    });
    let interrupt = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel.store(true, Ordering::SeqCst);
        }
    });
    let result = receiver
        .await
        .map_err(|_| anyhow!("the workflow engine stopped unexpectedly"));
    interrupt.abort();
    result?.map_err(|e| anyhow!("{e}"))
}

fn print_ending(outcome: &ExecOutcome, format: OutputFormat) {
    if format == OutputFormat::Jsonl {
        return;
    }
    let run = &outcome.report.run;
    match &outcome.report.ending {
        Ending::Succeeded(_) | Ending::Stopped(_) => {}
        Ending::AwaitingApproval(message) => {
            eprintln!(
                "\n{} {message}\n  Resume with: codemod workflow resume --id {} --approve",
                style("Waiting for approval:").yellow().bold(),
                run.id
            );
        }
        Ending::Failed(error) => {
            eprintln!(
                "\n{} {error}\n  Resume after fixing it with: codemod workflow resume --id {}",
                style("The run failed:").red().bold(),
                run.id
            );
        }
        Ending::Canceled => {
            eprintln!(
                "\n{} Resume with: codemod workflow resume --id {}",
                style("The run was canceled.").yellow().bold(),
                run.id
            );
        }
    }
}

// ----------------------------------------------------------- rendering ---

/// Renders engine events: step lines and progress bars for text, one JSON
/// object per line for JSONL.
pub struct Renderer {
    format: OutputFormat,
    multi: MultiProgress,
    bars: Mutex<HashMap<u64, ProgressBar>>,
    /// Streamed agent message text not yet followed by a full line.
    agent_text: Mutex<HashMap<u64, String>>,
    seq: AtomicU64,
}

impl Renderer {
    fn new(format: OutputFormat) -> Self {
        let multi = MultiProgress::new();
        if format == OutputFormat::Jsonl {
            multi.set_draw_target(ProgressDrawTarget::hidden());
        }
        Self {
            format,
            multi,
            bars: Mutex::new(HashMap::new()),
            agent_text: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(0),
        }
    }

    fn line(&self, text: String) {
        if self.multi.is_hidden() {
            // No terminal to draw on (piped or redirected): plain lines.
            eprintln!("{text}");
        } else {
            let _ = self.multi.println(text);
        }
    }

    fn flush_agent_text(&self, position: u64) {
        let text = self.agent_text.lock().unwrap().remove(&position);
        // Fragments carry their own line prefixes.
        if let Some(text) = text.filter(|t| !t.trim().is_empty()) {
            self.line(text.trim_end().to_string());
        }
    }

    fn finish(&self) {
        for (_, bar) in self.bars.lock().unwrap().drain() {
            bar.finish_and_clear();
        }
    }

    fn suspend<T>(&self, f: impl FnOnce() -> T) -> T {
        self.multi.suspend(f)
    }

    fn bar(&self, position: u64, total: usize) -> ProgressBar {
        let mut bars = self.bars.lock().unwrap();
        bars.entry(position)
            .or_insert_with(|| {
                let bar = self.multi.add(ProgressBar::new(total as u64));
                bar.set_style(
                    ProgressStyle::with_template("  {bar:30.cyan/blue} {pos}/{len} files")
                        .unwrap_or_else(|_| ProgressStyle::default_bar()),
                );
                bar
            })
            .clone()
    }
}

impl EventSink for Renderer {
    fn emit(&self, event: Event) {
        if self.format == OutputFormat::Jsonl {
            let mut record = serde_json::to_value(&event).unwrap_or(Value::Null);
            // Hold stdout while numbering, so lines come out in `seq` order even
            // when events arrive from several worker threads at once.
            let mut stdout = std::io::stdout().lock();
            if let Value::Object(map) = &mut record {
                map.insert("seq".into(), self.seq.fetch_add(1, Ordering::SeqCst).into());
                map.insert("ts".into(), chrono::Utc::now().to_rfc3339().into());
            }
            let _ = writeln!(stdout, "{record}");
            return;
        }
        match event {
            Event::RunStarted {
                run_id, resumed, ..
            } => self.line(format!(
                "{} {}",
                style(if resumed { "Resuming run" } else { "Run" }).bold(),
                style(run_id).dim()
            )),
            Event::StepStarted { label, .. } => self.line(format!("{} {label}", style("▸").cyan())),
            Event::StepReplayed { label, .. } => self.line(format!(
                "{} {label} {}",
                style("↺").dim(),
                style("(done earlier)").dim()
            )),
            Event::StepProgress {
                position,
                processed,
                total,
            } => {
                let bar = self.bar(position, total);
                bar.set_length(total as u64);
                bar.set_position(processed as u64);
            }
            Event::StepLog { line, .. } => self.line(format!("  {}", style(line).dim())),
            Event::RunLog { line } => self.line(style(line).dim().to_string()),
            Event::StepAgent {
                position, activity, ..
            } => {
                let line_open = self
                    .agent_text
                    .lock()
                    .unwrap()
                    .get(&position)
                    .is_some_and(|text| !text.is_empty() && !text.ends_with('\n'));
                match crate::agent_log_renderer::render_agent_activity(&activity, line_open) {
                    Some(crate::agent_log_renderer::RenderedAgentEvent::Line(line)) => {
                        self.flush_agent_text(position);
                        self.line(line);
                    }
                    Some(crate::agent_log_renderer::RenderedAgentEvent::Fragment(fragment)) => {
                        self.agent_text
                            .lock()
                            .unwrap()
                            .entry(position)
                            .or_default()
                            .push_str(&fragment);
                    }
                    None => {}
                }
            }
            Event::StepFinished {
                position,
                label,
                status,
                error,
                duration_ms,
            } => {
                if let Some(bar) = self.bars.lock().unwrap().remove(&position) {
                    bar.finish_and_clear();
                }
                self.flush_agent_text(position);
                let elapsed = format!("({:.1}s)", duration_ms as f64 / 1000.0);
                match status {
                    StepStatus::Succeeded => self.line(format!(
                        "{} {label} {}",
                        style("✓").green(),
                        style(elapsed).dim()
                    )),
                    StepStatus::Failed => self.line(format!(
                        "{} {label}: {}",
                        style("✗").red(),
                        error.unwrap_or_default()
                    )),
                }
            }
            Event::RunFinished { status, .. } => {
                if status == RunStatus::Succeeded {
                    self.line(format!("{}", style("Done").green().bold()));
                }
            }
        }
    }
}

// --------------------------------------------------------- interaction ---

struct TerminalInteraction {
    interactive: bool,
    approve_all: bool,
    renderer: Arc<Renderer>,
    /// Steps running at the same time must not prompt at the same time.
    prompt_lock: Mutex<()>,
}

impl Interaction for TerminalInteraction {
    fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn approve(&self, approval: &Approval) -> Decision {
        if self.approve_all || !self.interactive {
            return NonInteractive {
                approve_all: self.approve_all,
            }
            .approve(approval);
        }
        let _guard = self.prompt_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (question, on_no) = match approval {
            Approval::Manual { message } => (message.clone(), Decision::Deferred),
            Approval::ShellCommand { command } => {
                (format!("Run shell command: {command}?"), Decision::Denied)
            }
            Approval::Capabilities { modules } => (
                format!("Allow the transform to use {}?", modules.join(", ")),
                Decision::Denied,
            ),
            Approval::PullRequest { title, branch } => (
                format!("Push '{branch}' and open pull request '{title}'?"),
                Decision::Denied,
            ),
        };
        let answer = self
            .renderer
            .suspend(|| inquire::Confirm::new(&question).with_default(true).prompt());
        match answer {
            Ok(true) => Decision::Approved,
            Ok(false) => on_no,
            Err(_) => Decision::Deferred,
        }
    }

    fn choose_agent(&self, choices: &[AgentChoice]) -> Option<String> {
        if !self.interactive {
            return None;
        }
        let _guard = self.prompt_lock.lock().unwrap_or_else(|e| e.into_inner());
        let available: Vec<&AgentChoice> = choices.iter().filter(|c| c.available).collect();
        if available.is_empty() {
            return None;
        }
        let built_in = "Built-in agent".to_string();
        let mut options: Vec<String> = available.iter().map(|c| c.label.clone()).collect();
        options.push(built_in.clone());
        let picked = self.renderer.suspend(|| {
            inquire::Select::new("Which coding agent should run this step?", options).prompt()
        });
        match picked {
            Ok(label) if label != built_in => available
                .iter()
                .find(|c| c.label == label)
                .map(|c| c.name.clone()),
            _ => None,
        }
    }
}

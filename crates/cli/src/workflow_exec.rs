//! Runs workflows on the code-first engine (`codemod-workflow`) and renders
//! its events. Every command that runs or resumes a workflow goes through
//! here.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use butterflow_core::config::InstallSkillExecutor;
use butterflow_core::registry::RegistryClient;
use butterflow_core::structured_log::OutputFormat;
use butterflow_state::run_store::{store_from_env, RunRecord, RunStore};
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

/// The flags every command that runs or resumes a workflow takes.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct RunFlags {
    /// Allow fs access
    #[arg(long)]
    pub allow_fs: bool,

    /// Allow fetch access
    #[arg(long)]
    pub allow_fetch: bool,

    /// Allow child process access
    #[arg(long)]
    pub allow_child_process: bool,

    /// No interactive mode
    #[arg(long)]
    pub no_interactive: bool,

    /// Coding agent to use for AI steps (e.g. claude, codex, aider)
    #[arg(long)]
    pub agent: Option<String>,

    /// Execute install-skill steps when running in non-interactive mode
    #[arg(long)]
    pub install_skill: bool,

    /// Output format: "text" (default) or "jsonl" for structured logging
    #[arg(long, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
}

impl RunFlags {
    /// Capabilities granted with `--allow-*` flags.
    pub fn granted(&self) -> HashSet<LlrtSupportedModules> {
        [
            (self.allow_fs, LlrtSupportedModules::Fs),
            (self.allow_fetch, LlrtSupportedModules::Fetch),
            (self.allow_child_process, LlrtSupportedModules::ChildProcess),
        ]
        .into_iter()
        .filter_map(|(allowed, module)| allowed.then_some(module))
        .collect()
    }

    /// The capabilities a run gets: the flags', and those the package asks
    /// for (in its codemod.yaml, given or found in `workflow_dir`) that the
    /// person grants when asked.
    pub fn capabilities(
        &self,
        manifest: Option<crate::utils::manifest::CodemodManifest>,
        workflow_dir: Option<PathBuf>,
        dry_run: bool,
    ) -> HashSet<LlrtSupportedModules> {
        use crate::utils::resolve_capabilities::{
            prompt_capabilities, resolve_capabilities, ResolveCapabilitiesArgs,
        };
        let requested = resolve_capabilities(
            ResolveCapabilitiesArgs {
                allow_fs: self.allow_fs,
                allow_fetch: self.allow_fetch,
                allow_child_process: self.allow_child_process,
            },
            manifest,
            workflow_dir,
        );
        prompt_capabilities(requested, &self.granted(), self.no_interactive, dry_run)
    }

    /// Settings for a run with these flags and nothing else special.
    pub fn settings(
        &self,
        capabilities: HashSet<LlrtSupportedModules>,
        registry: RegistryClient,
        telemetry: &crate::TelemetrySenderMutex,
    ) -> ExecSettings {
        ExecSettings {
            no_interactive: self.no_interactive,
            format: self.format,
            capabilities,
            agent: self.agent.clone(),
            registry: Some(registry),
            // Unattended runs install skills only when asked to.
            install_skill: (!self.no_interactive || self.install_skill).then(|| {
                crate::commands::package_skill::create_install_skill_executor(telemetry.clone())
            }),
            approve_all: false,
            managed_git: false,
            skip_shard_steps: false,
            nested_observer: None,
        }
    }
}

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
pub fn store() -> Arc<dyn RunStore> {
    Arc::from(store_from_env())
}

/// A run's status for display. A run whose record says running while
/// nothing executes it was interrupted: its process was killed.
pub fn status_label(store: &dyn RunStore, run: &RunRecord) -> String {
    let interrupted = run.status == RunStatus::Running && !store.is_held(&run.id).unwrap_or(true);
    if interrupted {
        "interrupted".to_string()
    } else {
        run.status.as_str().to_string()
    }
}

pub async fn start(request: ExecRequest) -> Result<ExecOutcome> {
    let workflow = load_workflow(&request.workflow).map_err(|e| anyhow!("{e}"))?;
    let format = request.settings.format;
    let dry_run = request.dry_run;
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
    // A dry run is not saved, so there is nothing to resume.
    print_ending(&outcome, format, !dry_run);
    Ok(outcome)
}

/// Continue a run with the settings it was started with, plus what
/// `settings` adds: capabilities granted and git use carry over, so the rest
/// of the run behaves like its start.
pub async fn resume(run_id: &str, mut settings: ExecSettings) -> Result<ExecOutcome> {
    let record = store()
        .get_run(run_id)?
        .ok_or_else(|| anyhow!("run '{run_id}' not found"))?;
    settings.capabilities.extend(
        record
            .settings
            .capabilities
            .iter()
            .filter_map(|name| name.parse::<LlrtSupportedModules>().ok()),
    );
    settings.managed_git |= record.settings.managed_git;
    settings.agent = settings.agent.or(record.settings.agent);

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
    print_ending(&outcome, format, true);
    Ok(outcome)
}

fn engine(settings: &ExecSettings) -> Result<(Engine, Arc<Renderer>, Arc<Summary>)> {
    let renderer = Arc::new(Renderer::new(settings.format));
    let interaction: Arc<dyn Interaction> = Arc::new(TerminalInteraction {
        interactive: crate::utils::interactive::is_interactive(settings.no_interactive),
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
        summary: Arc::clone(&summary),
    };
    let engine = Engine::new(store(), renderer.clone(), interaction, services);
    Ok((engine, renderer, summary))
}

/// Run the engine on its own thread (its tasks start their own async
/// runtimes), stopping its run when this process is asked to stop.
async fn run_off_thread<F>(engine: Engine, run: F) -> Result<RunReport>
where
    F: FnOnce(&Engine) -> Result<RunReport, codemod_workflow::EngineError> + Send + 'static,
{
    let stop = engine.stop_handle();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(run(&engine));
    });
    let interrupt = tokio::spawn(async move {
        stop_requested().await;
        stop.stop();
    });
    let result = receiver
        .await
        .map_err(|_| anyhow!("the workflow engine stopped unexpectedly"));
    interrupt.abort();
    result?.map_err(|e| anyhow!("{e}"))
}

/// Resolves when the process is asked to stop: Ctrl-C, or being terminated
/// or hung up on (a supervisor, a closed terminal). The run then ends as
/// canceled and can be resumed, instead of being left marked as running.
async fn stop_requested() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut terminate), Ok(mut hangup)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::hangup()),
        ) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
            _ = hangup.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn print_ending(outcome: &ExecOutcome, format: OutputFormat, resumable: bool) {
    if format == OutputFormat::Jsonl {
        return;
    }
    let run = &outcome.report.run;
    let hint = |command: String| {
        if resumable {
            format!("\n  {command}")
        } else {
            String::new()
        }
    };
    match &outcome.report.ending {
        Ending::Succeeded(_) | Ending::Stopped(_) => {}
        Ending::AwaitingApproval(message) => {
            eprintln!(
                "\n{} {message}{}",
                style("Waiting for approval:").yellow().bold(),
                hint(format!(
                    "Resume with: codemod workflow resume --id {} --approve",
                    run.id
                ))
            );
        }
        Ending::Failed(error) => {
            eprintln!(
                "\n{} {error}{}",
                style("The run failed:").red().bold(),
                hint(format!(
                    "Resume after fixing it with: codemod workflow resume --id {}",
                    run.id
                ))
            );
        }
        Ending::Canceled => {
            eprintln!(
                "\n{}{}",
                style("The run was canceled.").yellow().bold(),
                hint(format!(
                    "Resume with: codemod workflow resume --id {}",
                    run.id
                ))
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
                    StepStatus::Canceled => {
                        self.line(format!("{} {label}: canceled", style("■").yellow()))
                    }
                    StepStatus::AwaitingApproval => self.line(format!(
                        "{} {label}: waiting for approval",
                        style("‖").yellow()
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
        let grants_capabilities = matches!(approval, Approval::Capabilities { .. });
        if !self.interactive || (self.approve_all && !grants_capabilities) {
            return NonInteractive {
                approve_all: self.approve_all,
            }
            .approve(approval);
        }
        let _guard = self.prompt_lock.lock().unwrap_or_else(|e| e.into_inner());
        let question = match approval {
            Approval::Manual { message } => message.clone(),
            Approval::ShellCommand { command } => format!("Run shell command: {command}?"),
            Approval::Capabilities { modules } => {
                format!("Allow the transform to use {}?", modules.join(", "))
            }
            Approval::PullRequest { title, branch } => {
                format!("Push '{branch}' and open pull request '{title}'?")
            }
        };
        // Enter never approves: every one of these runs or publishes something.
        let answer = self.renderer.suspend(|| {
            inquire::Confirm::new(&question)
                .with_default(false)
                .prompt()
        });
        match (answer, approval) {
            (Ok(true), _) => Decision::Approved,
            // A manual approval can be given later; the rest are answered now.
            (_, Approval::Manual { .. }) => Decision::Deferred,
            _ => Decision::Denied,
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

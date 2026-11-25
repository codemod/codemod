//! Runs workflows on the code-first engine (`codemod-workflow`) and renders
//! its events. Every command that runs or resumes a workflow goes through
//! here.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::user_settings::DecisionModel;
use anyhow::{anyhow, Result};
use butterflow_core::config::InstallSkillExecutor;
use butterflow_core::registry::RegistryClient;
use butterflow_core::structured_log::OutputFormat;
use butterflow_state::run_store::{store_from_env, RunRecord, RunStore};
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use codemod_workflow::events::{Event, EventSink};
use codemod_workflow::interaction::{
    AgentChoice, Approval, Ask, Decision, Interaction, NonInteractive,
};
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

    /// Allow codemod/assess to send files to the decision model (System One
    /// by default): asked for interactively, required without a terminal
    #[arg(long)]
    pub allow_ai: bool,

    /// No interactive mode
    #[arg(long)]
    pub no_interactive: bool,

    /// Coding agent to use for AI steps (e.g. claude, codex, aider); defaults to the one saved
    /// in the launcher
    #[arg(long)]
    pub agent: Option<String>,

    /// Decision model for decision and assessment calls, as <provider>/<model> (default:
    /// the one saved in the launcher, else systemone/jev-latest)
    #[arg(long, value_name = "PROVIDER/MODEL", value_parser = parse_decision_model)]
    pub model: Option<DecisionModel>,

    /// Execute install-skill steps when running in non-interactive mode
    #[arg(long)]
    pub install_skill: bool,

    /// Output format: "text" (default) or "jsonl" for structured logging
    #[arg(long, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Let another program drive the run: "stdio" reads JSON commands on stdin (answer, approve,
    /// reject, stop) and waits at questions instead of exiting. Needs --format jsonl
    #[arg(long, value_enum, value_name = "CHANNEL")]
    pub control: Option<crate::control::ControlMode>,
}

fn parse_decision_model(text: &str) -> Result<DecisionModel> {
    DecisionModel::parse(text)
}

/// Flags for starting a run that `workflow run` and `run` (registry
/// packages) share; `workflow resume` has its own `--approve`.
#[derive(clap::Args, Debug, Default, Clone)]
pub struct StartFlags {
    /// Give every approval the workflow asks for (manual steps, pull requests)
    #[arg(long)]
    pub approve: bool,

    /// Let branch, commit and pull request steps change the git repository
    #[arg(long)]
    pub managed_git: bool,
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
            allow_ai: self.allow_ai,
            agent: self.agent.clone(),
            decision_model: self.model.clone(),
            decision: None,
            registry: Some(registry),
            // Unattended runs install skills only when asked to.
            install_skill: (!self.no_interactive || self.install_skill).then(|| {
                crate::commands::package_skill::create_install_skill_executor(telemetry.clone())
            }),
            approve_all: false,
            approve_ids: Vec::new(),
            managed_git: false,
            skip_shard_steps: false,
            nested_observer: None,
            launcher: crate::launcher::enabled(self),
            allow_dirty: false,
            control: self.control,
            force: false,
        }
    }
}

/// Settings shared by starting and resuming a run.
pub struct ExecSettings {
    pub no_interactive: bool,
    pub format: OutputFormat,
    pub capabilities: HashSet<LlrtSupportedModules>,
    /// `--allow-ai`: codemod/assess may send files to the decision model.
    pub allow_ai: bool,
    pub agent: Option<String>,
    /// The decision model `--model` asked for; filled in from the saved
    /// settings when the run starts.
    pub decision_model: Option<DecisionModel>,
    /// What answers `codemod/assess`: set by the check before the run
    /// starts, `None` when the workflow does not use it.
    pub decision: Option<Arc<dyn codemod_workflow::assess::DecisionModel>>,
    pub registry: Option<RegistryClient>,
    pub install_skill: Option<Arc<dyn InstallSkillExecutor>>,
    /// Treat every approval as given.
    pub approve_all: bool,
    /// Treat these questions as approved (`workflow resume --approve <id>`).
    pub approve_ids: Vec<String>,
    /// Let `branch`, `commit` and `pullRequest` steps touch git.
    pub managed_git: bool,
    pub skip_shard_steps: bool,
    pub nested_observer:
        Option<Arc<dyn butterflow_core::nested_codemod_run::NestedCodemodRunObserver>>,
    /// Show the run in the interactive launcher instead of printing it.
    pub launcher: bool,
    /// Applying a dry run for real from the launcher skips the check for
    /// uncommitted changes.
    pub allow_dirty: bool,
    /// `--control`: another program drives the run.
    pub control: Option<crate::control::ControlMode>,
    /// `workflow resume --force`: continue a canceled or rejected run.
    pub force: bool,
}

impl ExecSettings {
    pub fn approvals(&self) -> crate::control::Approvals {
        crate::control::Approvals {
            all: self.approve_all,
            ids: self.approve_ids.clone(),
        }
    }

    /// A control channel speaks JSONL.
    fn check_control(&self) -> Result<()> {
        if self.control.is_some() && self.format != OutputFormat::Jsonl {
            return Err(anyhow!(
                "--control stdio needs --format jsonl: events go to stdout as JSON lines"
            ));
        }
        Ok(())
    }

    fn hello_settings(&self) -> crate::control::HelloSettings {
        let mut capabilities: Vec<String> = self
            .capabilities
            .iter()
            .map(|capability| capability.as_str().to_string())
            .collect();
        capabilities.sort();
        crate::control::HelloSettings {
            capabilities,
            managed_git: self.managed_git,
            agent: self.agent.clone(),
            decision_model: self.decision_model.as_ref().map(DecisionModel::id),
            approve_all: self.approve_all,
            approved: self.approve_ids.clone(),
        }
    }

    /// Fill in what the person saved (coding agent, decision model) where
    /// the flags say nothing. `recorded_agent` is the one a resumed run was
    /// started with, which comes before the saved one.
    fn with_saved(mut self, recorded_agent: Option<String>) -> Self {
        let saved = crate::user_settings::load();
        self.agent = self.agent.or(recorded_agent).or_else(|| saved.agent(None));
        self.decision_model = Some(saved.decision_model(self.decision_model.as_ref()));
        self
    }
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
    /// Whether the run was a dry run: a launcher's dry run may have been
    /// applied for real since the command started.
    pub dry_run: bool,
    pub format: OutputFormat,
}

impl ExecOutcome {
    pub fn files_modified(&self) -> usize {
        self.summary.stats.files_modified.load(Ordering::Relaxed)
    }

    pub fn files_unmodified(&self) -> usize {
        self.summary.stats.files_unmodified.load(Ordering::Relaxed)
    }

    /// Files that failed: in the run's partial steps, or as counted while
    /// the run progressed, whichever is more.
    pub fn files_with_errors(&self) -> usize {
        self.summary
            .stats
            .files_with_errors
            .load(Ordering::Relaxed)
            .max(self.failed_files())
    }

    /// How many files failed in the run's partial steps.
    pub fn failed_files(&self) -> usize {
        self.report.failed_files()
    }

    /// `Ok` when the run finished or is waiting for an approval, even if
    /// some files failed. Failures were already shown while the run
    /// progressed. A dry run that stopped at an approval is not `Ok`: it
    /// is not saved, so it cannot wait to be resumed.
    pub fn ended(&self) -> Result<()> {
        match &self.report.ending {
            Ending::AwaitingApproval(_) if self.dry_run => {
                Err(WorkflowFailureAlreadyReported.into())
            }
            Ending::Succeeded(_) | Ending::AwaitingApproval(_) | Ending::Stopped(_) => Ok(()),
            Ending::Failed(_) | Ending::Canceled | Ending::Paused | Ending::Rejected(_) => {
                Err(WorkflowFailureAlreadyReported.into())
            }
            Ending::FailedWithOutput { exit_code, .. } => Err(FailedByWorkflow {
                exit_code: *exit_code,
            }
            .into()),
        }
    }

    /// Like [`Self::ended`], but a run that finished with failed files is
    /// an error too: [`FinishedWithFailedFiles`], exit code 2.
    pub fn result(&self) -> Result<()> {
        self.ended()?;
        match self.failed_files() {
            0 => Ok(()),
            failed => Err(FinishedWithFailedFiles {
                failed,
                dry_run: self.dry_run,
                quiet: self.format == OutputFormat::Jsonl,
            }
            .into()),
        }
    }
}

/// The exit code of a run that finished, but with some files failed. A run
/// that failed outright exits with 1.
pub const EXIT_FAILED_FILES: i32 = 2;

/// A run finished, but some of its files failed. The CLI ends its output
/// with this and exits with [`EXIT_FAILED_FILES`].
#[derive(Debug)]
pub struct FinishedWithFailedFiles {
    pub failed: usize,
    pub dry_run: bool,
    /// The output is JSONL, where `run_finished` already says it.
    pub quiet: bool,
}

impl std::fmt::Display for FinishedWithFailedFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let files = if self.failed == 1 { "file" } else { "files" };
        let rest = if self.dry_run {
            "dry run: nothing was written"
        } else {
            "changes to the other files were written"
        };
        write!(f, "{} {files} failed; {rest}.", self.failed)
    }
}

impl std::error::Error for FinishedWithFailedFiles {}

/// The workflow called `fail()`: the run failed with an output, and the CLI
/// exits with the code it gave (2 unless it said otherwise). The run's output
/// already described it.
#[derive(Debug)]
pub struct FailedByWorkflow {
    pub exit_code: i32,
}

impl std::fmt::Display for FailedByWorkflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the workflow failed (exit code {})", self.exit_code)
    }
}

impl std::error::Error for FailedByWorkflow {}

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

pub async fn start(mut request: ExecRequest) -> Result<ExecOutcome> {
    request.settings.check_control()?;
    // `--model` (or the launcher's choice), before the saved one fills it in.
    let chosen = request.settings.decision_model.clone();
    request.settings = request.settings.with_saved(None);
    let workflow = load_workflow(&request.workflow).map_err(|e| anyhow!("{e}"))?;
    // Before the launcher's screen opens: it may ask in the terminal.
    request.settings.decision = crate::assess_preflight::decision_model(
        &workflow.path,
        chosen.as_ref(),
        request.settings.allow_ai,
        crate::utils::interactive::is_interactive(request.settings.no_interactive),
        &crate::assess_preflight::TerminalPrompts,
    )?;
    if request.settings.launcher {
        return crate::launcher::run_start(request).await;
    }
    let format = request.settings.format;
    let dry_run = request.dry_run;
    let allow_dirty = request.settings.allow_dirty;
    // Picked here, so `hello` can name the run before it starts.
    let run_id = uuid::Uuid::new_v4().to_string();
    if request.settings.control.is_some() {
        crate::control::Hello {
            event: "hello",
            protocol: crate::control::PROTOCOL,
            cli: crate::CLI_VERSION,
            run_id: run_id.clone(),
            mode: "run",
            dry_run,
            workflow: workflow.path.clone(),
            target: request.target.clone(),
            settings: request.settings.hello_settings(),
            commands: crate::control::COMMANDS.to_vec(),
        }
        .print();
    }
    // Dry runs are not saved, so they keep no journal.
    let (engine, renderer, summary) =
        engine(&request.settings, (!dry_run).then(|| run_id.clone()))?;
    let started = Instant::now();
    let report = run_off_thread(engine, move |engine| {
        engine.start(StartRequest {
            run_id: Some(run_id),
            workflow,
            target: request.target,
            params: Value::Object(request.params.into_iter().collect()),
            name: request.name,
            dry_run: request.dry_run,
            test: None,
            // Before the first write, not now: a run that only reads is
            // never stopped by uncommitted changes.
            check_uncommitted: !allow_dirty,
        })
    })
    .await?;
    renderer.finish();
    let outcome = ExecOutcome {
        report,
        summary,
        duration: started.elapsed(),
        dry_run,
        format,
    };
    // A dry run is not saved, so there is nothing to resume.
    print_ending(&outcome, format, !dry_run);
    Ok(outcome)
}

/// Continue a run with the settings it was started with, plus what
/// `settings` adds: capabilities granted and git use carry over, so the rest
/// of the run behaves like its start.
pub async fn resume(run_id: &str, mut settings: ExecSettings) -> Result<ExecOutcome> {
    settings.check_control()?;
    let record = store()
        .get_run(run_id)?
        .ok_or_else(|| anyhow!("run '{run_id}' not found"))?;
    // Before asking anything: a run that is over stays over.
    match record.status.resumable() {
        codemod_workflow::Resumable::Yes => {}
        codemod_workflow::Resumable::WithForce if settings.force => {}
        codemod_workflow::Resumable::WithForce => {
            return Err(codemod_workflow::EngineError::Ended {
                run: run_id.to_string(),
                status: record.status,
            }
            .into())
        }
        codemod_workflow::Resumable::No => {
            return Err(codemod_workflow::EngineError::AlreadyFinished(run_id.to_string()).into())
        }
    }
    settings.capabilities.extend(
        record
            .settings
            .capabilities
            .iter()
            .filter_map(|name| name.parse::<LlrtSupportedModules>().ok()),
    );
    settings.managed_git |= record.settings.managed_git;
    let chosen = settings.decision_model.clone();
    let mut settings = settings.with_saved(record.settings.agent.clone());
    // Asked again: a key given for the start is not kept.
    settings.decision = crate::assess_preflight::decision_model(
        &record.workflow_path,
        chosen.as_ref(),
        settings.allow_ai,
        crate::utils::interactive::is_interactive(settings.no_interactive),
        &crate::assess_preflight::TerminalPrompts,
    )?;
    if settings.launcher {
        let title = record.name.clone().unwrap_or_else(|| {
            record
                .workflow_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| run_id.to_string())
        });
        return crate::launcher::run_resume(run_id, title, settings).await;
    }

    let format = settings.format;
    if settings.control.is_some() {
        crate::control::Hello {
            event: "hello",
            protocol: crate::control::PROTOCOL,
            cli: crate::CLI_VERSION,
            run_id: run_id.to_string(),
            mode: "resume",
            dry_run: false,
            workflow: record.workflow_path.clone(),
            target: record.target.clone(),
            settings: settings.hello_settings(),
            commands: crate::control::COMMANDS.to_vec(),
        }
        .print();
    }
    let (engine, renderer, summary) = engine(&settings, Some(run_id.to_string()))?;
    let started = Instant::now();
    let run_id = run_id.to_string();
    let options = codemod_workflow::ResumeOptions {
        allow_dirty: settings.allow_dirty,
        force: settings.force,
    };
    let report = run_off_thread(engine, move |engine| engine.resume_with(&run_id, options)).await?;
    renderer.finish();
    let outcome = ExecOutcome {
        report,
        summary,
        duration: started.elapsed(),
        // A dry run is not saved, so it is never resumed.
        dry_run: false,
        format,
    };
    print_ending(&outcome, format, true);
    Ok(outcome)
}

/// An engine whose events are numbered, kept in the journal of `journal`
/// (the run's id; `None` for a dry run) and rendered; it asks over the
/// control channel when there is one, else in the terminal.
fn engine(
    settings: &ExecSettings,
    journal: Option<String>,
) -> Result<(Engine, Arc<Renderer>, Arc<Summary>)> {
    let renderer = Arc::new(Renderer::new(settings.format));
    let events = Arc::new(crate::journal::Recorder::new(
        store(),
        journal,
        renderer.clone(),
    ));
    let control = settings.control.map(|_| {
        Arc::new(crate::control::Control::new(
            events.clone(),
            settings.approvals(),
        ))
    });
    let interaction: Arc<dyn Interaction> = match &control {
        Some(control) => control.clone(),
        None => Arc::new(TerminalInteraction {
            interactive: crate::utils::interactive::is_interactive(settings.no_interactive),
            approvals: settings.approvals(),
            renderer: Arc::clone(&renderer),
            prompt_lock: Mutex::new(()),
        }),
    };
    let (engine, summary) = engine_with(settings, events, interaction);
    if let Some(control) = control {
        control.listen(engine.stop_handle());
    }
    Ok((engine, renderer, summary))
}

/// An engine for these settings that reports to `events` and asks
/// `interaction`.
pub(crate) fn engine_with(
    settings: &ExecSettings,
    events: Arc<dyn EventSink>,
    interaction: Arc<dyn Interaction>,
) -> (Engine, Arc<Summary>) {
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
        decision: settings.decision.clone(),
    };
    (Engine::new(store(), events, interaction, services), summary)
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
pub(crate) async fn stop_requested() {
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

pub(crate) fn print_ending(outcome: &ExecOutcome, format: OutputFormat, resumable: bool) {
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
        Ending::FailedWithOutput { error, .. } => {
            eprintln!("\n{} {error}", style("The run failed:").red().bold());
        }
        Ending::AwaitingApproval(message) if outcome.dry_run => {
            eprintln!(
                "\n{} {message}\n  A dry run is not saved, so it cannot wait to be resumed. Run it again with --approve to go past approvals.",
                style("The dry run stopped at an approval:").yellow().bold(),
            );
        }
        Ending::AwaitingApproval(message) => {
            let approve = match &run.question {
                Some(question) => format!("--approve {question}"),
                None => "--approve".to_string(),
            };
            eprintln!(
                "\n{} {message}{}",
                style("Waiting for approval:").yellow().bold(),
                hint(format!(
                    "Approve it and go on with: codemod workflow resume --id {} {approve}",
                    run.id
                ))
            );
        }
        Ending::Rejected(message) => {
            eprintln!(
                "\n{} {message}\n  The run ended; what earlier steps changed was kept.",
                style("Rejected:").red().bold(),
            );
        }
        Ending::Paused => {
            eprintln!(
                "\n{}{}",
                style("The run was paused.").yellow().bold(),
                hint(format!(
                    "Resume with: codemod workflow resume --id {}",
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
            eprintln!("\n{}", style("The run was canceled.").yellow().bold());
        }
    }
    if let Some(line) = output_summary(outcome.report.ending.output(), &run.id, resumable) {
        eprintln!("{line}");
    }
}

/// How much of a run's output the text format shows.
const OUTPUT_SUMMARY_CHARS: usize = 200;

/// `Output: <JSON>` for a run that returned something, cut short when long,
/// with where to read all of it. `saved`: the run is in the run store.
fn output_summary(output: &Value, run_id: &str, saved: bool) -> Option<String> {
    if output.is_null() {
        return None;
    }
    let json = output.to_string();
    if json.chars().count() <= OUTPUT_SUMMARY_CHARS {
        return Some(format!("{} {json}", style("Output:").bold()));
    }
    let cut: String = json.chars().take(OUTPUT_SUMMARY_CHARS).collect();
    let full = if saved {
        format!("codemod workflow status --id {run_id} --format json, or ")
    } else {
        String::new()
    };
    Some(format!(
        "{} {cut}…\n  {}",
        style("Output:").bold(),
        style(format!(
            "The whole output: {full}`run_finished.output` with --format jsonl"
        ))
        .dim()
    ))
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
    /// A dry run's diff has begun.
    diffs_shown: AtomicBool,
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
            diffs_shown: AtomicBool::new(false),
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

impl crate::journal::Stamped for Renderer {
    fn emit(&self, event: Event, record: &Value) {
        if self.format == OutputFormat::Jsonl {
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{record}");
            let _ = stdout.flush();
            return;
        }
        // A nested codemod's steps show as their parent step's.
        let Some(event) = event.top_level() else {
            return;
        };
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
            Event::StepRetrying {
                label,
                attempt,
                attempts,
                error,
                ..
            } => self.line(format!(
                "{} {label}: attempt {attempt} of {attempts} failed: {error}",
                style("↻").yellow()
            )),
            Event::StepProgress {
                at,
                processed,
                total,
            } => {
                let bar = self.bar(at.position, total);
                bar.set_length(total as u64);
                bar.set_position(processed as u64);
            }
            Event::StepLog { line, .. } => self.line(format!("  {}", style(line).dim())),
            Event::RunLog { line, .. } => self.line(style(line).dim().to_string()),
            Event::StepAgent { at, activity, .. } => {
                let position = at.position;
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
                at,
                label,
                status,
                error,
                duration_ms,
                ..
            } => {
                let position = at.position;
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
                    StepStatus::Partial => self.line(format!(
                        "{} {label}: {} {}",
                        style("◐").yellow(),
                        error.unwrap_or_default(),
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
                    StepStatus::Rejected => {
                        self.line(format!("{} {label}: rejected", style("✗").red()))
                    }
                    // Never reported: a step that began to wait has not finished.
                    StepStatus::Waiting => {}
                }
            }
            // Real runs' diffs are for JSONL and the launcher; questions are
            // only asked over a control channel, which speaks JSONL.
            Event::StepDiff { .. }
            | Event::Question { .. }
            | Event::QuestionClosed { .. }
            | Event::ControlError { .. } => {}
            Event::RunDiff { diff, .. } => {
                if !self.diffs_shown.swap(true, Ordering::SeqCst) {
                    self.line(format!(
                        "\n{}",
                        style("Dry run: these changes were not written").bold()
                    ));
                }
                self.line(color_diff(&diff));
            }
            Event::RunFinished {
                status,
                failed_files,
                partial_steps,
                ..
            } => {
                if status == RunStatus::Succeeded {
                    match (failed_files, partial_steps.len()) {
                        (0, _) => self.line(format!("{}", style("Done").green().bold())),
                        (files, steps) => self.line(format!(
                            "{} {}",
                            style("Done").yellow().bold(),
                            style(format!(
                                "({files} {} failed in {steps} {})",
                                if files == 1 { "file" } else { "files" },
                                if steps == 1 { "step" } else { "steps" }
                            ))
                            .yellow()
                        )),
                    }
                }
            }
        }
    }
}

/// A unified diff with its headers, hunks, additions and removals styled.
fn color_diff(diff: &str) -> String {
    // Only the line breaks: trailing spaces on the last line are part of it.
    diff.trim_end_matches(['\n', '\r'])
        .lines()
        .map(|line| {
            if line.starts_with("+++") || line.starts_with("---") {
                style(line).bold().to_string()
            } else if line.starts_with('+') {
                style(line).green().to_string()
            } else if line.starts_with('-') {
                style(line).red().to_string()
            } else if line.starts_with("@@") {
                style(line).cyan().to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// --------------------------------------------------------- interaction ---

struct TerminalInteraction {
    interactive: bool,
    approvals: crate::control::Approvals,
    renderer: Arc<Renderer>,
    /// Steps running at the same time must not prompt at the same time.
    prompt_lock: Mutex<()>,
}

impl Interaction for TerminalInteraction {
    fn is_interactive(&self) -> bool {
        self.interactive
    }

    fn ask(&self, ask: &Ask, approval: &Approval) -> Decision {
        if self.approvals.gives(&ask.id, approval) {
            return Decision::Approved;
        }
        self.approve(approval)
    }

    fn approve(&self, approval: &Approval) -> Decision {
        let approve_all = self.approvals.all;
        if !self.interactive || (approve_all && approval.given_by_approve_all()) {
            return NonInteractive { approve_all }.approve(approval);
        }
        let _guard = self.prompt_lock.lock().unwrap_or_else(|e| e.into_inner());
        let question = approval.prompt();
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

    fn choose(&self, _ask: &Ask, message: &str, options: &[String]) -> Option<String> {
        if !self.interactive {
            return None;
        }
        let _guard = self.prompt_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.renderer
            .suspend(|| inquire::Select::new(message, options.to_vec()).prompt())
            .ok()
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

#[cfg(test)]
mod color_diff_tests {
    use super::color_diff;

    #[test]
    fn trailing_spaces_on_the_last_line_are_kept() {
        let shown = color_diff("-a\n+a \n");
        assert_eq!(console::strip_ansi_codes(&shown), "-a\n+a ");
    }
}

//! `codemod workflow run -w workflow.ts`: TypeScript workflows on the
//! orchestration engine. The workflow graph is bundled with the portable
//! runtime (`orchestration-build`, cached by the content of every input
//! file) and run in one QuickJS context by `orchestration-quickjs-host`, in
//! this process. SIGINT, SIGTERM and SIGHUP abort the run (the host cancels
//! the operation in flight). The final output is printed as JSON on stdout;
//! workflow console output goes to stderr.
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use orchestration_build::{cached_quickjs_bundle, default_bundle_cache, Runtime};
use orchestration_quickjs_host::{run, HostOptions};

/// Set when the runner was not found next to this binary.
const AGENT_BIN_ENV: &str = "CODEMOD_AGENT_BIN";

/// True for a workflow the orchestration engine runs.
pub fn is_typescript_workflow(workflow: &str) -> bool {
    matches!(
        Path::new(workflow).extension().and_then(|e| e.to_str()),
        Some("ts" | "mts")
    )
}

pub struct Options<'a> {
    pub workflow: &'a str,
    pub target: Option<&'a Path>,
    /// The root input as JSON text.
    pub input: Option<&'a str>,
    /// Resumed from when it exists; the run's history is written back to it.
    pub history: Option<&'a Path>,
}

/// The portable runtime's sources. `CODEMOD_ORCHESTRATION_SRC`, else this
/// checkout's `packages/orchestration/src`.
fn runtime() -> Result<Runtime> {
    let src = std::env::var_os("CODEMOD_ORCHESTRATION_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/orchestration/src")
        });
    let src = src.canonicalize().with_context(|| {
        format!(
            "the orchestration runtime sources are missing at {}; set CODEMOD_ORCHESTRATION_SRC",
            src.display()
        )
    })?;
    Ok(Runtime { src })
}

/// `codemod-agent`, which `agent` steps spawn, is installed next to this binary.
fn agent_binary() -> Option<PathBuf> {
    let candidate = std::env::current_exe()
        .ok()?
        .parent()?
        .join("codemod-agent");
    candidate.is_file().then_some(candidate)
}

pub async fn run_typescript_workflow(options: Options<'_>) -> Result<()> {
    let started = Instant::now();
    let workflow = Path::new(options.workflow)
        .canonicalize()
        .with_context(|| format!("workflow does not exist: {}", options.workflow))?;
    let target = match options.target {
        Some(target) => target.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let target = target
        .canonicalize()
        .with_context(|| format!("target does not exist: {}", target.display()))?;
    if let Some(input) = options.input {
        if let Err(error) = serde_json::from_str::<serde_json::Value>(input) {
            bail!("--input must be valid JSON ({error}); got: {input}");
        }
    }
    let history = match options.history {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => bail!("failed to read history {}: {error}", path.display()),
        },
        None => None,
    };

    let cache = default_bundle_cache().map_err(anyhow::Error::msg)?;
    let bundle = cached_quickjs_bundle(&workflow, &runtime()?, &cache).await?;

    if std::env::var_os(AGENT_BIN_ENV).is_none() {
        if let Some(agent) = agent_binary() {
            // SAFETY: set before the host thread starts; nothing else reads
            // the environment concurrently at this point.
            std::env::set_var(AGENT_BIN_ENV, agent);
        }
    }

    let host = HostOptions {
        bundle: bundle.path,
        target,
        input: options.input.map(str::to_owned),
        history,
    };
    // The host drives its own current-thread runtime.
    let outcome = tokio::task::spawn_blocking(move || run(host, started))
        .await
        .context("the workflow host stopped unexpectedly")?
        .map_err(anyhow::Error::msg)?;
    let result = outcome.result;

    if let (Some(path), Some(history)) = (options.history, result.get("history")) {
        std::fs::write(path, history.to_string())
            .with_context(|| format!("failed to write history {}", path.display()))?;
    }
    if result.get("ok").and_then(|ok| ok.as_bool()) != Some(true) {
        let error = result
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or("the workflow failed");
        bail!("{error}");
    }
    let output = result.get("output").cloned().unwrap_or_default();
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

//! `install({ dir, lockfileOnly })` from `codemod/deps`: run the
//! repository's package manager so the lockfile catches up with the
//! manifests. Opt-in, a step of its own; a dry run does not run it.
//!
//! It never waits on a prompt or a dead network for long: stdin is closed,
//! the npm-family fetch settings fail fast, and a package manager that is not
//! installed fails at once. The step's `timeout` still applies.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use butterflow_runners::direct_runner::DirectRunner;
use butterflow_runners::Runner;
use codemod_deps::project::{self, PackageManagerName, Project};
use serde::Deserialize;
use serde_json::json;

use super::{block_on, child_env, TaskContext, TaskError, TaskOutcome, TaskResult};
use crate::interaction::{Approval, Decision};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Request {
    /// The package directory to run in, relative to the target.
    #[serde(default)]
    dir: Option<String>,
    #[serde(default = "lockfile_only_default")]
    lockfile_only: bool,
}

fn lockfile_only_default() -> bool {
    true
}

/// The lockfile each package manager writes when there is none yet.
fn default_lockfile(name: PackageManagerName) -> &'static str {
    match name {
        PackageManagerName::Npm => "package-lock.json",
        PackageManagerName::Pnpm => "pnpm-lock.yaml",
        PackageManagerName::Yarn => "yarn.lock",
        PackageManagerName::Bun => "bun.lock",
    }
}

/// Whether `program` is on PATH.
fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let names: Vec<String> = if cfg!(windows) {
        ["", ".exe", ".cmd", ".bat"]
            .iter()
            .map(|ext| format!("{program}{ext}"))
            .collect()
    } else {
        vec![program.to_string()]
    };
    std::env::split_paths(&path).any(|dir| names.iter().any(|name| dir.join(name).is_file()))
}

pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    let run = task.run;
    let root: &Path = if run.target.is_file() {
        run.target.parent().unwrap_or(&run.target)
    } else {
        &run.target
    };
    let relative = match request.dir.as_deref() {
        None | Some("") | Some(".") => ".".to_string(),
        Some(dir) => super::changes::normalize(dir).ok_or_else(|| {
            TaskError::Failed(format!(
                "install: dir '{dir}' must be a path inside the target"
            ))
        })?,
    };
    let dir: PathBuf = super::changes::under(root, &relative);
    let present = project::present_root_files(&dir);
    let read = |path: &str| std::fs::read_to_string(dir.join(path)).ok();
    let project = Project::read(&[], &present, &read);
    let manager = project.package_manager.ok_or_else(|| {
        TaskError::Failed(format!(
            "install: no package.json in {relative}, so there is nothing to install"
        ))
    })?;
    let mut command = manager.install_command(request.lockfile_only);
    // pnpm freezes the lockfile in CI; the point here is to update it.
    if manager.name == PackageManagerName::Pnpm {
        command.push_str(" --no-frozen-lockfile");
    }
    let lockfile = manager
        .lockfile
        .clone()
        .unwrap_or_else(|| default_lockfile(manager.name).to_string());
    let lockfile_path = dir.join(&lockfile);
    let shown = if relative == "." {
        lockfile.clone()
    } else {
        format!("{relative}/{lockfile}")
    };
    let result = |ran: bool, changed: bool| {
        json!({
            "packageManager": manager.name.as_str(),
            "command": command,
            "ran": ran,
            "lockfile": shown,
            "changed": changed,
        })
    };

    if run.dry_run {
        task.log(format!(
            "dry run: would run {command} in {relative} (the lockfile is not refreshed)"
        ));
        return Ok(TaskOutcome::new(result(false, false)));
    }
    match run.interaction.approve(&Approval::ShellCommand {
        command: command.clone(),
    }) {
        Decision::Approved => {}
        Decision::Denied => return Err(TaskError::Failed(format!("{command} was not approved"))),
        Decision::Deferred => {
            return Err(TaskError::AwaitingApproval(format!(
                "approve running: {command}"
            )))
        }
    }
    let cloud = butterflow_core::git_ops::is_cloud_mode();
    let cloud_hint = if cloud {
        "; cloud runs reach only registry.npmjs.org and GitHub unless your organization allows more hosts"
    } else {
        ""
    };
    let program = manager.name.as_str();
    if !on_path(program) {
        return Err(TaskError::Failed(format!(
            "install: {program} is not installed on this machine (the repository uses {program}, from {}){cloud_hint}",
            manager.detected_from
        )));
    }

    let before = std::fs::read(&lockfile_path).ok();
    let mut env = child_env(task);
    for (key, value) in [
        ("npm_config_fetch_retries", "1"),
        ("npm_config_fetch_timeout", "60000"),
        ("npm_config_fetch_retry_maxtimeout", "10000"),
        ("YARN_HTTP_TIMEOUT", "60000"),
        ("YARN_HTTP_RETRY", "1"),
        ("YARN_ENABLE_IMMUTABLE_INSTALLS", "false"),
        ("YARN_ENABLE_INTERACTIVE", "0"),
    ] {
        env.entry(key.to_string())
            .or_insert_with(|| value.to_string());
    }
    task.log(format!("running {command} in {relative}"));
    let events = Arc::clone(&run.events);
    let at = task.at();
    let on_line: butterflow_runners::OutputCallback = Arc::new(move |line: String| {
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
        .with_cwd(dir.clone());
    let output =
        block_on(runner.run_command(&command, &env, Some(on_line))).map_err(|e| match e {
            butterflow_models::Error::Canceled => TaskError::Canceled,
            other => TaskError::Failed(format!("install: {other}")),
        })?;
    if output.exit_code != 0 {
        let printed = if output.stderr.trim().is_empty() {
            &output.stdout
        } else {
            &output.stderr
        };
        let lines: Vec<&str> = printed.trim_end().lines().collect();
        let tail = lines[lines.len().saturating_sub(15)..].join("\n");
        return Err(TaskError::Failed(format!(
            "install: {command} exited with code {}{cloud_hint}:\n{tail}",
            output.exit_code
        )));
    }
    let after = std::fs::read(&lockfile_path).ok();
    let changed = before != after;
    task.log(if changed {
        format!("{shown} updated")
    } else {
        format!("{shown} unchanged")
    });
    Ok(TaskOutcome::new(result(true, changed)))
}

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
    if !dir.join("package.json").is_file() {
        if let Some(python) = python_manager(&dir, request.lockfile_only) {
            return run_python(task, &dir, &relative, python);
        }
    }
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
    // Where the package manager will fetch from: the package's own config,
    // else the target's.
    let registries = project::registries(&manager, &|path| {
        std::fs::read_to_string(dir.join(path))
            .or_else(|_| std::fs::read_to_string(root.join(path)))
            .ok()
    });
    let program = manager.name.as_str();
    if !on_path(program) {
        let hint = if cloud {
            cloud_hint(&registries, "")
        } else {
            String::new()
        };
        return Err(TaskError::Failed(format!(
            "install: {program} is not installed on this machine (the repository uses {program}, from {}){hint}",
            manager.detected_from
        )));
    }
    if cloud {
        for (host, from) in registries.iter().filter(|(host, _)| !cloud_allows(host)) {
            task.log(format!(
                "cloud run: {program} fetches from {host} ({}), which cloud runs reach only when your organization allows it",
                described(from)
            ));
        }
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
        let hint = if cloud {
            cloud_hint(&registries, printed)
        } else {
            String::new()
        };
        return Err(TaskError::Failed(format!(
            "install: {command} exited with code {}:\n{tail}{hint}",
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

/// Hosts a cloud run reaches without its organization adding them: the
/// default egress allowlist of Codemod's cloud runner (codemod-app,
/// `egress-policy.ts`). `*.` matches any subdomain.
const CLOUD_ALLOWED: [&str; 8] = [
    "registry.npmjs.org",
    "*.npmjs.com",
    "github.com",
    "*.github.com",
    "*.githubusercontent.com",
    "*.githubassets.com",
    "codemod.com",
    "*.codemod.com",
];

/// Whether a cloud run reaches `host` without its organization allowing it.
fn cloud_allows(host: &str) -> bool {
    CLOUD_ALLOWED
        .iter()
        .any(|allowed| match allowed.strip_prefix("*.") {
            Some(domain) => host == domain || host.ends_with(&format!(".{domain}")),
            None => host == *allowed,
        })
}

/// "Yarn's default", or "from .npmrc".
fn described(from: &str) -> String {
    if from.ends_with("default") {
        from.to_string()
    } else {
        format!("from {from}")
    }
}

/// What a failed install in a cloud run adds to its error: the registries
/// the package manager uses (host, and where it comes from) and the hosts
/// the command's output names that cloud runs cannot reach by default; or
/// the general rule when there are none.
fn cloud_hint(registries: &[(String, String)], output: &str) -> String {
    let mut blocked: Vec<String> = registries
        .iter()
        .filter(|(host, _)| !cloud_allows(host))
        .map(|(host, from)| format!("{host} ({})", described(from)))
        .collect();
    let named = regex::Regex::new(
        r"(?i)(?:https?://|ENOTFOUND |EAI_AGAIN |ECONNREFUSED |getaddrinfo \w+ )([a-z0-9][a-z0-9.-]*\.[a-z]{2,})",
    )
    .expect("valid regex");
    for found in named.captures_iter(output) {
        let host = found[1].to_ascii_lowercase();
        let known = blocked.iter().any(|b| b.starts_with(&format!("{host} ")));
        if !cloud_allows(&host) && !known {
            blocked.push(format!("{host} (in the command's output)"));
        }
    }
    let rule = "This is a cloud run: it reaches only registry.npmjs.org and GitHub unless your organization allows more hosts";
    if blocked.is_empty() {
        format!("\n{rule}.")
    } else {
        format!(
            "\n{rule}, and the install needs {}. Allow {} in your organization's egress settings, or point the registry at registry.npmjs.org.",
            blocked.join(", "),
            if blocked.len() == 1 { "it" } else { "them" }
        )
    }
}

/// A Python project's package manager: program, command, lockfile.
struct PythonManager {
    name: &'static str,
    command: String,
    lockfile: Option<&'static str>,
}

/// uv (uv.lock or `[tool.uv]`), Poetry (poetry.lock or `[tool.poetry]`),
/// else pip with a requirements file (no lockfile: only a full install runs).
fn python_manager(dir: &Path, lockfile_only: bool) -> Option<PythonManager> {
    let pyproject = std::fs::read_to_string(dir.join("pyproject.toml")).ok();
    let has = |table: &str| {
        pyproject
            .as_deref()
            .is_some_and(|text| text.lines().any(|l| l.trim().starts_with(table)))
    };
    if dir.join("uv.lock").is_file() || has("[tool.uv") {
        return Some(PythonManager {
            name: "uv",
            command: if lockfile_only { "uv lock" } else { "uv sync" }.to_string(),
            lockfile: Some("uv.lock"),
        });
    }
    if dir.join("poetry.lock").is_file() || has("[tool.poetry") {
        return Some(PythonManager {
            name: "poetry",
            command: if lockfile_only {
                "poetry lock --no-interaction"
            } else {
                "poetry install --no-interaction"
            }
            .to_string(),
            lockfile: Some("poetry.lock"),
        });
    }
    if pyproject.is_some() && dir.join("pylock.toml").is_file() {
        return Some(PythonManager {
            name: "uv",
            command: "uv lock".to_string(),
            lockfile: Some("uv.lock"),
        });
    }
    let requirements = ["requirements.txt", "requirements/base.txt"]
        .into_iter()
        .find(|file| dir.join(file).is_file());
    if let Some(file) = requirements {
        return Some(PythonManager {
            name: "pip",
            command: if lockfile_only {
                String::new()
            } else {
                format!("pip install -r {file}")
            },
            lockfile: None,
        });
    }
    pyproject.map(|_| PythonManager {
        name: "uv",
        command: if lockfile_only { "uv lock" } else { "uv sync" }.to_string(),
        lockfile: Some("uv.lock"),
    })
}

fn run_python(
    task: &TaskContext<'_>,
    dir: &Path,
    relative: &str,
    manager: PythonManager,
) -> TaskResult {
    let run = task.run;
    let shown = manager.lockfile.map(|lockfile| {
        if relative == "." {
            lockfile.to_string()
        } else {
            format!("{relative}/{lockfile}")
        }
    });
    let result = |ran: bool, changed: bool| {
        json!({
            "packageManager": manager.name,
            "command": manager.command,
            "ran": ran,
            "lockfile": shown,
            "changed": changed,
        })
    };
    if manager.command.is_empty() {
        task.log(format!(
            "{}: no lockfile to refresh in {relative} (pass lockfileOnly: false to install)",
            manager.name
        ));
        return Ok(TaskOutcome::new(result(false, false)));
    }
    if run.dry_run {
        task.log(format!(
            "dry run: would run {} in {relative} (the lockfile is not refreshed)",
            manager.command
        ));
        return Ok(TaskOutcome::new(result(false, false)));
    }
    match run.interaction.approve(&Approval::ShellCommand {
        command: manager.command.clone(),
    }) {
        Decision::Approved => {}
        Decision::Denied => {
            return Err(TaskError::Failed(format!(
                "{} was not approved",
                manager.command
            )))
        }
        Decision::Deferred => {
            return Err(TaskError::AwaitingApproval(format!(
                "approve running: {}",
                manager.command
            )))
        }
    }
    if !on_path(manager.name) {
        return Err(TaskError::Failed(format!(
            "install: {} is not installed on this machine",
            manager.name
        )));
    }
    let lockfile_path = manager.lockfile.map(|l| dir.join(l));
    let before = lockfile_path.as_ref().and_then(|p| std::fs::read(p).ok());
    let env = child_env(task);
    task.log(format!("running {} in {relative}", manager.command));
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
        .with_cwd(dir.to_path_buf());
    let output = block_on(runner.run_command(&manager.command, &env, Some(on_line))).map_err(
        |e| match e {
            butterflow_models::Error::Canceled => TaskError::Canceled,
            other => TaskError::Failed(format!("install: {other}")),
        },
    )?;
    if output.exit_code != 0 {
        let printed = if output.stderr.trim().is_empty() {
            &output.stdout
        } else {
            &output.stderr
        };
        let lines: Vec<&str> = printed.trim_end().lines().collect();
        let tail = lines[lines.len().saturating_sub(15)..].join("\n");
        return Err(TaskError::Failed(format!(
            "install: {} exited with code {}:\n{tail}",
            manager.command, output.exit_code
        )));
    }
    let after = lockfile_path.as_ref().and_then(|p| std::fs::read(p).ok());
    Ok(TaskOutcome::new(result(true, before != after)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(host: &str, from: &str) -> (String, String) {
        (host.to_string(), from.to_string())
    }

    #[test]
    fn a_cloud_failure_names_the_registry_cloud_runs_cannot_reach() {
        assert_eq!(
            cloud_hint(&[registry("registry.yarnpkg.com", "Yarn's default")], "YN0001: fetch failed"),
            "\nThis is a cloud run: it reaches only registry.npmjs.org and GitHub unless your organization allows more hosts, and the install needs registry.yarnpkg.com (Yarn's default). Allow it in your organization's egress settings, or point the registry at registry.npmjs.org."
        );
        assert_eq!(
            cloud_hint(
                &[
                    registry("registry.npmjs.org", "npm's default"),
                    registry("npm.acme.dev", ".npmrc")
                ],
                "npm error request to https://npm.acme.dev/@acme%2fui failed\nnpm error getaddrinfo ENOTFOUND cdn.acme.dev"
            ),
            "\nThis is a cloud run: it reaches only registry.npmjs.org and GitHub unless your organization allows more hosts, and the install needs npm.acme.dev (from .npmrc), cdn.acme.dev (in the command's output). Allow them in your organization's egress settings, or point the registry at registry.npmjs.org."
        );
    }

    #[test]
    fn a_cloud_failure_with_allowed_registries_states_the_rule() {
        assert_eq!(
            cloud_hint(
                &[registry("registry.npmjs.org", "npm's default")],
                "npm error 404 https://registry.npmjs.org/nope and https://codeload.github.com/x"
            ),
            "\nThis is a cloud run: it reaches only registry.npmjs.org and GitHub unless your organization allows more hosts."
        );
        assert!(cloud_allows("objects.githubusercontent.com"));
        assert!(!cloud_allows("evilgithub.com"));
    }
}

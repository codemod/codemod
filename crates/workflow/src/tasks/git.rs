//! `branch`, `commit` and `pullRequest`. They only touch git when managed
//! git is enabled (as in the YAML engine); otherwise they are recorded as
//! skipped.

use butterflow_core::git_ops;
use serde::Deserialize;
use serde_json::json;

use super::{block_on, TaskContext, TaskError, TaskOutcome, TaskResult};
use crate::interaction::{Approval, Decision};

fn skipped(task: &TaskContext<'_>) -> Option<TaskResult> {
    if task.run.dry_run || !task.run.services.managed_git {
        task.log("skipped: managed git is off");
        return Some(Ok(TaskOutcome::new(json!({ "skipped": true }))));
    }
    None
}

fn git_error(error: impl std::fmt::Display) -> TaskError {
    TaskError::Failed(error.to_string())
}

#[derive(Deserialize)]
pub(crate) struct Branch {
    name: String,
}

pub(crate) fn branch(task: &TaskContext<'_>, request: Branch) -> TaskResult {
    if let Some(result) = skipped(task) {
        return result;
    }
    block_on(git_ops::checkout_branch(&request.name, &task.run.target)).map_err(git_error)?;
    Ok(TaskOutcome::new(json!({ "branch": request.name })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Commit {
    message: String,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    allow_empty: bool,
}

pub(crate) fn commit(task: &TaskContext<'_>, request: Commit) -> TaskResult {
    if let Some(result) = skipped(task) {
        return result;
    }
    let committed = block_on(git_ops::commit(
        &request.message,
        &request.add,
        request.allow_empty,
        &task.run.target,
    ))
    .map_err(git_error)?;
    Ok(TaskOutcome::new(json!({ "committed": committed })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRequest {
    title: String,
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    base: Option<String>,
    /// Defaults to the current branch.
    branch: Option<String>,
}

pub(crate) fn pull_request(task: &TaskContext<'_>, request: PullRequest) -> TaskResult {
    if let Some(result) = skipped(task) {
        return result;
    }
    let target = &task.run.target;
    let branch = match request.branch {
        Some(branch) => branch,
        None => current_branch(target)?,
    };
    match task.run.interaction.approve(&Approval::PullRequest {
        title: request.title.clone(),
        branch: branch.clone(),
    }) {
        Decision::Approved => {}
        Decision::Denied => return Ok(TaskOutcome::new(json!({ "skipped": true }))),
        Decision::Deferred => {
            return Err(TaskError::AwaitingApproval(format!(
                "approve opening pull request '{}'",
                request.title
            )))
        }
    }
    block_on(git_ops::push_branch(&branch, target)).map_err(git_error)?;
    let url = block_on(git_ops::create_pull_request(
        &request.title,
        request.body.as_deref(),
        request.draft,
        &branch,
        request.base.as_deref(),
        &format!("{}:{}", task.run.run_id, task.position),
        target,
    ))
    .map_err(git_error)?;
    if let Some(url) = &url {
        task.log(format!("Pull request created: {url}"));
    }
    Ok(TaskOutcome::new(json!({ "branch": branch, "url": url })))
}

fn current_branch(dir: &std::path::Path) -> Result<String, TaskError> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .map_err(git_error)?;
    if !output.status.success() {
        return Err(TaskError::Failed(
            "cannot read the current git branch".into(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

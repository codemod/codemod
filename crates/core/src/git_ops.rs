//! Git operations for cloud-mode commit checkpoints and PR creation.

use log::{debug, info, warn};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::process::Command;

use butterflow_models::variable::TaskExpressionContext;
use butterflow_models::Result;

/// Returns `true` when the engine is running in cloud mode
/// (i.e. `BUTTERFLOW_STATE_BACKEND=cloud`).
pub fn is_cloud_mode() -> bool {
    std::env::var("BUTTERFLOW_STATE_BACKEND")
        .map(|v| v == "cloud")
        .unwrap_or(false)
}

/// Build a [`TaskExpressionContext`] from a task ID.
///
/// In addition to computing the task signature, this reads all environment
/// variables prefixed with `CODEMOD_TASK_` (excluding `CODEMOD_TASK_ID`) and
/// exposes them as `task.<lowercase_suffix>` template variables.
pub fn build_task_expression_context(task_id: &str) -> TaskExpressionContext {
    let signature = compute_task_signature(task_id);
    let extra = collect_task_env_vars();
    TaskExpressionContext {
        id: task_id.to_string(),
        signature,
        extra,
    }
}

/// Collect `CODEMOD_TASK_*` environment variables (excluding `CODEMOD_TASK_ID`)
/// into a map keyed by the lowercased suffix.
///
/// Example: `CODEMOD_TASK_JIRA_TITLE=Fix bug` → `("jira_title", "Fix bug")`
pub fn collect_task_env_vars() -> std::collections::HashMap<String, String> {
    let mut extra = std::collections::HashMap::new();
    for (key, value) in std::env::vars() {
        if let Some(suffix) = key.strip_prefix("CODEMOD_TASK_") {
            // Skip CODEMOD_TASK_ID — it's already handled as task.id
            if suffix == "ID" {
                continue;
            }
            extra.insert(suffix.to_lowercase(), value);
        }
    }
    extra
}

/// SHA-256 the task id and return the first 8 hex characters.
/// Matches the TypeScript implementation:
/// ```js
/// crypto.createHash("sha256").update(taskId).digest("hex").slice(0, 8)
/// ```
pub fn compute_task_signature(task_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(task_id.as_bytes());
    let hash = hasher.finalize();
    // First 4 bytes = 8 hex chars
    hash[..4]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>()
}

/// Resolve the branch name for the current node.
/// If `configured_branch_name` is `Some`, it is used as-is (already template-resolved).
/// Otherwise falls back to `codemod-{signature}`.
pub fn resolve_branch_name(configured_branch_name: Option<&str>, task_signature: &str) -> String {
    match configured_branch_name {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => format!("codemod-{}", task_signature),
    }
}

/// Checkout a new branch. Equivalent to `git checkout -b <branch>`.
pub async fn checkout_branch(branch: &str, working_dir: &std::path::Path) -> Result<()> {
    info!("Checking out branch: {}", branch);
    // Use -B to force-create (or reset) the branch, making retries idempotent.
    let output = Command::new("git")
        .args(["checkout", "-B", branch])
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|e| butterflow_models::Error::Runtime(format!("git checkout -B failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(butterflow_models::Error::Runtime(format!(
            "git checkout -B {} failed: {}",
            branch, stderr
        )));
    }
    Ok(())
}

/// Returns `true` when the working tree has uncommitted changes
/// (staged or unstaged).
pub async fn has_changes(working_dir: &std::path::Path) -> Result<bool> {
    // Check unstaged
    let unstaged = Command::new("git")
        .args(["diff", "--quiet"])
        .current_dir(working_dir)
        .status()
        .await
        .map_err(|e| butterflow_models::Error::Runtime(format!("git diff failed: {e}")))?;

    if !unstaged.success() {
        return Ok(true);
    }

    // Check staged
    let staged = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(working_dir)
        .status()
        .await
        .map_err(|e| butterflow_models::Error::Runtime(format!("git diff --cached failed: {e}")))?;

    if !staged.success() {
        return Ok(true);
    }

    // Check untracked files
    let untracked = Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|e| butterflow_models::Error::Runtime(format!("git ls-files failed: {e}")))?;

    let has_untracked = !untracked.stdout.is_empty();
    Ok(has_untracked)
}

/// Stage files and create a commit.
/// `paths` defaults to `["."]` if empty.
/// Returns `true` if a commit was actually created (i.e. there were changes).
pub async fn commit(
    message: &str,
    paths: &[String],
    allow_empty: bool,
    working_dir: &std::path::Path,
) -> Result<bool> {
    let add_paths: Vec<&str> = if paths.is_empty() {
        vec!["."]
    } else {
        paths.iter().map(|s| s.as_str()).collect()
    };

    // git add
    let mut add_cmd = Command::new("git");
    add_cmd.arg("add").args(&add_paths).current_dir(working_dir);
    let add_output = add_cmd
        .output()
        .await
        .map_err(|e| butterflow_models::Error::Runtime(format!("git add failed: {e}")))?;
    if !add_output.status.success() {
        let stderr = String::from_utf8_lossy(&add_output.stderr);
        return Err(butterflow_models::Error::Runtime(format!(
            "git add failed: {stderr}"
        )));
    }

    // Check if there's anything staged
    let staged_check = Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(working_dir)
        .status()
        .await
        .map_err(|e| butterflow_models::Error::Runtime(format!("git diff --cached failed: {e}")))?;

    if staged_check.success() {
        // Nothing staged
        if allow_empty {
            debug!("No changes staged, skipping commit (allow_empty=true)");
            return Ok(false);
        } else {
            return Err(butterflow_models::Error::Runtime(
                "No changes staged for commit".to_string(),
            ));
        }
    }

    // git commit
    let commit_output = Command::new("git")
        .args(["commit", "--no-verify", "-m", message])
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|e| butterflow_models::Error::Runtime(format!("git commit failed: {e}")))?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        return Err(butterflow_models::Error::Runtime(format!(
            "git commit failed: {stderr}"
        )));
    }

    info!("Created commit: {}", message);
    Ok(true)
}

/// Detect the remote default branch (e.g. "main" or "master").
///
/// If the `CODEMOD_BASE_BRANCH` environment variable is set and non-empty,
/// its value is used directly without any git detection.
async fn detect_remote_base_branch(working_dir: &std::path::Path) -> String {
    // Honour explicit override from the environment
    if let Ok(branch) = std::env::var("CODEMOD_BASE_BRANCH") {
        if !branch.is_empty() {
            return branch;
        }
    }

    // Try symbolic-ref first
    if let Ok(output) = Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD", "--short"])
        .current_dir(working_dir)
        .output()
        .await
    {
        if output.status.success() {
            let branch = String::from_utf8_lossy(&output.stdout)
                .trim()
                .trim_start_matches("origin/")
                .to_string();
            if !branch.is_empty() {
                return branch;
            }
        }
    }

    // Fallback: try common default branch names on the remote.
    for candidate in &["main", "master"] {
        if let Ok(output) = Command::new("git")
            .args([
                "ls-remote",
                "--exit-code",
                "origin",
                &format!("refs/heads/{candidate}"),
            ])
            .current_dir(working_dir)
            .output()
            .await
        {
            if output.status.success() {
                return candidate.to_string();
            }
        }
    }

    "main".to_string()
}

/// Push the branch to origin with retry logic.
/// Matches the TypeScript implementation: 3 attempts with exponential backoff,
/// plus a remote verification fallback.
pub async fn push_branch(branch: &str, working_dir: &std::path::Path) -> Result<()> {
    let max_retries = 3u32;
    let mut pushed = false;

    for attempt in 1..=max_retries {
        let push_output = Command::new("git")
            .args([
                "-c",
                "http.version=HTTP/1.1",
                "-c",
                "http.postBuffer=524288000",
                "push",
                "--verbose",
                "origin",
                branch,
                "--force",
            ])
            .current_dir(working_dir)
            .output()
            .await
            .map_err(|e| butterflow_models::Error::Runtime(format!("git push failed: {e}")))?;

        debug!(
            "Push stdout: {}",
            String::from_utf8_lossy(&push_output.stdout)
        );
        debug!(
            "Push stderr (last 2000 chars): {}",
            String::from_utf8_lossy(&push_output.stderr)
                .chars()
                .rev()
                .take(2000)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        );

        if push_output.status.success() {
            pushed = true;
            break;
        }

        warn!(
            "Push attempt {}/{} failed (exit code {:?})",
            attempt,
            max_retries,
            push_output.status.code()
        );

        if attempt < max_retries {
            let delay = Duration::from_millis((attempt as u64) * 2000);
            info!("Retrying in {}s...", delay.as_secs());
            tokio::time::sleep(delay).await;
        }
    }

    // Verify branch exists on remote even if push reported failure
    if !pushed {
        let verify = Command::new("git")
            .args([
                "-c",
                "http.version=HTTP/1.1",
                "ls-remote",
                "--exit-code",
                "origin",
                &format!("refs/heads/{}", branch),
            ])
            .current_dir(working_dir)
            .output()
            .await
            .map_err(|e| butterflow_models::Error::Runtime(format!("git ls-remote failed: {e}")))?;

        if verify.status.success() {
            info!("Push reported failure but branch exists on remote — continuing.");
        } else {
            return Err(butterflow_models::Error::Runtime(
                "Failed to push changes and branch does not exist on remote.".to_string(),
            ));
        }
    }

    info!("Pushed branch to origin: {}", branch);
    Ok(())
}

/// Create a pull request via the Codemod API.
/// `task_id` is the workflow task UUID (available as `task.id` in the engine).
/// Returns the PR URL on success, if the API response includes one.
pub async fn create_pull_request(
    title: &str,
    body: Option<&str>,
    draft: bool,
    head: &str,
    base: Option<&str>,
    task_id: &str,
    working_dir: &std::path::Path,
) -> Result<Option<String>> {
    let api_endpoint = std::env::var("BUTTERFLOW_API_ENDPOINT").map_err(|_| {
        butterflow_models::Error::Runtime(
            "BUTTERFLOW_API_ENDPOINT environment variable is required".to_string(),
        )
    })?;
    let auth_token = std::env::var("BUTTERFLOW_API_AUTH_TOKEN").map_err(|_| {
        butterflow_models::Error::Runtime(
            "BUTTERFLOW_API_AUTH_TOKEN environment variable is required".to_string(),
        )
    })?;

    let resolved_base = match base {
        Some(b) if !b.is_empty() => b.to_string(),
        _ => detect_remote_base_branch(working_dir).await,
    };

    let mut pr_data = serde_json::json!({
        "title": title,
        "head": head,
        "base": resolved_base,
        "body": body.unwrap_or(""),
    });

    if draft {
        pr_data["draft"] = serde_json::Value::Bool(true);
    }

    let pr_url = format!(
        "{}/api/butterflow/v1/tasks/{}/pull-request",
        api_endpoint, task_id
    );

    info!("Creating pull request...");
    info!("  URL: {}", pr_url);
    info!("  Title: {}", title);
    info!("  Head: {}", head);
    info!("  Base: {}", resolved_base);

    let client = reqwest::Client::new();
    let response = client
        .post(&pr_url)
        .header("Authorization", format!("Bearer {}", auth_token))
        .header("Content-Type", "application/json")
        .json(&pr_data)
        .send()
        .await
        .map_err(|e| {
            butterflow_models::Error::Runtime(format!("Failed to create pull request: {e}"))
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(butterflow_models::Error::Runtime(format!(
            "Failed to create pull request: HTTP {} - {}",
            status, error_text
        )));
    }

    // Extract the PR URL from the response if available
    let pr_url = response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|body| body.get("url").and_then(|v| v.as_str()).map(String::from));

    match &pr_url {
        Some(url) => info!("Pull request created: {}", url),
        None => info!("Pull request created successfully!"),
    }

    Ok(pr_url)
}

//! Git operations for cloud-mode commit checkpoints and PR creation.

use log::{debug, info, warn};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

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

fn worktree_operation_lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn sanitize_path_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => ch,
            _ => '-',
        })
        .collect();

    let trimmed = sanitized.trim_start_matches('.');
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "worktree".to_string()
    } else {
        trimmed.to_string()
    }
}

fn redact_git_credentials(value: &str) -> String {
    let mut redacted = String::with_capacity(value.len());
    let mut remaining = value;

    while let Some(scheme_index) = remaining.find("://") {
        let authority_start = scheme_index + 3;
        redacted.push_str(&remaining[..authority_start]);

        let after_scheme = &remaining[authority_start..];
        let url_end = after_scheme
            .find(|ch: char| {
                ch.is_whitespace() || ch == '\'' || ch == '"' || ch == '<' || ch == '>'
            })
            .unwrap_or(after_scheme.len());
        let url_without_scheme = &after_scheme[..url_end];

        if let Some(userinfo_end) = url_without_scheme.rfind('@') {
            redacted.push_str("<redacted>@");
            redacted.push_str(&url_without_scheme[userinfo_end + 1..]);
        } else {
            redacted.push_str(url_without_scheme);
        }

        remaining = &after_scheme[url_end..];
    }

    redacted.push_str(remaining);
    redacted
}

#[cfg(unix)]
fn detach_from_controlling_terminal(command: &mut Command) {
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn detach_from_controlling_terminal(_command: &mut Command) {}

fn configure_non_interactive_git_command(command: &mut Command) {
    detach_from_controlling_terminal(command);
    command.stdin(Stdio::null()).env("GIT_TERMINAL_PROMPT", "0");
}

fn configure_non_interactive_github_command(command: &mut Command) {
    configure_non_interactive_git_command(command);
    command
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1");
}

fn is_authentication_prompt_or_failure(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("username for 'https://github.com'")
        || normalized.contains("password for 'https://")
        || normalized.contains("could not read username")
        || normalized.contains("could not read password")
        || normalized.contains("terminal prompts disabled")
        || normalized.contains("authentication failed")
        || normalized.contains("could not authenticate")
        || normalized.contains("requires authentication")
        || normalized.contains("gh auth login")
        || normalized.contains("not logged into any github hosts")
}

fn github_auth_guidance(action: &str) -> String {
    format!(
        "GitHub authentication is required to {action}. Log in first with `gh auth login`, or configure git credentials for `origin`, then retry."
    )
}

fn normalize_git_push_error(stderr_summary: &str) -> String {
    if is_authentication_prompt_or_failure(stderr_summary) {
        github_auth_guidance("push the branch")
    } else {
        stderr_summary.to_string()
    }
}

fn normalize_gh_pr_create_error(stderr: &str, stdout: &str) -> String {
    let details = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };

    if is_authentication_prompt_or_failure(details) {
        github_auth_guidance("create the pull request")
    } else {
        details.to_string()
    }
}

pub async fn repo_root(working_dir: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(working_dir)
        .output()
        .await
        .map_err(|e| butterflow_models::Error::Runtime(format!("git rev-parse failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(butterflow_models::Error::Runtime(format!(
            "git rev-parse --show-toplevel failed: {stderr}"
        )));
    }

    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

pub fn worktree_path(repo_root: &Path, branch: &str, task_id: &str) -> PathBuf {
    let repo_name = repo_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo");
    let container = repo_root
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{repo_name}.codemod-worktrees"));
    container.join(format!(
        "{}-{}",
        sanitize_path_component(branch),
        sanitize_path_component(task_id)
    ))
}

pub async fn create_worktree(repo_root: &Path, branch: &str, task_id: &str) -> Result<PathBuf> {
    let _lock = worktree_operation_lock().lock().await;
    let worktree_path = worktree_path(repo_root, branch, task_id);

    if let Some(parent) = worktree_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            butterflow_models::Error::Runtime(format!(
                "failed to create worktree parent directory: {e}"
            ))
        })?;
    }

    if worktree_path.exists() {
        let _ = remove_worktree(repo_root, &worktree_path).await;
    }

    let output = Command::new("git")
        .args([
            "worktree",
            "add",
            "--force",
            "-B",
            branch,
            worktree_path.to_string_lossy().as_ref(),
            "HEAD",
        ])
        .current_dir(repo_root)
        .output()
        .await
        .map_err(|e| butterflow_models::Error::Runtime(format!("git worktree add failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(butterflow_models::Error::Runtime(format!(
            "git worktree add failed for branch {}: {}",
            branch, stderr
        )));
    }

    Ok(worktree_path)
}

pub async fn remove_worktree(repo_root: &Path, worktree_path: &Path) -> Result<()> {
    let _lock = worktree_operation_lock().lock().await;
    let output = Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            worktree_path.to_string_lossy().as_ref(),
        ])
        .current_dir(repo_root)
        .output()
        .await
        .map_err(|e| {
            butterflow_models::Error::Runtime(format!("git worktree remove failed: {e}"))
        })?;

    if !output.status.success() && worktree_path.exists() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(butterflow_models::Error::Runtime(format!(
            "git worktree remove failed: {stderr}"
        )));
    }

    if worktree_path.exists() {
        tokio::fs::remove_dir_all(worktree_path)
            .await
            .map_err(|e| {
                butterflow_models::Error::Runtime(format!(
                    "failed to remove worktree directory {}: {e}",
                    worktree_path.display()
                ))
            })?;
    }

    Ok(())
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
    let mut symbolic_ref = Command::new("git");
    symbolic_ref
        .args(["symbolic-ref", "refs/remotes/origin/HEAD", "--short"])
        .current_dir(working_dir);
    configure_non_interactive_git_command(&mut symbolic_ref);
    if let Ok(output) = symbolic_ref.output().await {
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
        let mut command = Command::new("git");
        command
            .args([
                "ls-remote",
                "--exit-code",
                "origin",
                &format!("refs/heads/{candidate}"),
            ])
            .current_dir(working_dir);
        configure_non_interactive_git_command(&mut command);
        if let Ok(output) = command.output().await {
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
    let mut last_push_stderr: Option<String> = None;

    for attempt in 1..=max_retries {
        let mut command = Command::new("git");
        command
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
            .current_dir(working_dir);
        configure_non_interactive_git_command(&mut command);

        let push_output = command
            .output()
            .await
            .map_err(|e| butterflow_models::Error::Runtime(format!("git push failed: {e}")))?;

        let push_stdout = redact_git_credentials(&String::from_utf8_lossy(&push_output.stdout));
        let push_stderr = redact_git_credentials(&String::from_utf8_lossy(&push_output.stderr));
        debug!("Push stdout: {}", push_stdout);
        debug!(
            "Push stderr (last 2000 chars): {}",
            push_stderr
                .chars()
                .rev()
                .take(2000)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
        );
        let stderr_summary = push_stderr
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .unwrap_or("git push failed")
            .to_string();
        let normalized_stderr_summary = normalize_git_push_error(&stderr_summary);

        if push_output.status.success() {
            pushed = true;
            break;
        }

        last_push_stderr = Some(normalized_stderr_summary.clone());

        warn!(
            "Push attempt {}/{} failed (exit code {:?}): {}",
            attempt,
            max_retries,
            push_output.status.code(),
            normalized_stderr_summary
        );

        if attempt < max_retries {
            let delay = Duration::from_millis((attempt as u64) * 2000);
            info!("Retrying in {}s...", delay.as_secs());
            tokio::time::sleep(delay).await;
        }
    }

    // Verify branch exists on remote even if push reported failure
    if !pushed {
        let mut verify_command = Command::new("git");
        verify_command
            .args([
                "-c",
                "http.version=HTTP/1.1",
                "ls-remote",
                "--exit-code",
                "origin",
                &format!("refs/heads/{}", branch),
            ])
            .current_dir(working_dir);
        configure_non_interactive_git_command(&mut verify_command);
        let verify = verify_command
            .output()
            .await
            .map_err(|e| butterflow_models::Error::Runtime(format!("git ls-remote failed: {e}")))?;

        if verify.status.success() {
            info!("Push reported failure but branch exists on remote — continuing.");
        } else {
            let remote = Command::new("git")
                .args(["remote", "get-url", "origin"])
                .current_dir(working_dir)
                .output()
                .await
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .map(|stdout| redact_git_credentials(stdout.trim()))
                .filter(|url| !url.is_empty())
                .unwrap_or_else(|| "origin".to_string());
            return Err(butterflow_models::Error::Runtime(
                match last_push_stderr {
                    Some(stderr) => format!(
                        "Failed to push branch '{branch}' to {remote}: {stderr}"
                    ),
                    None => format!(
                        "Failed to push branch '{branch}' to {remote}; branch does not exist on remote."
                    ),
                },
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
    let resolved_base = match base {
        Some(b) if !b.is_empty() => b.to_string(),
        _ => detect_remote_base_branch(working_dir).await,
    };

    let api_endpoint = std::env::var("BUTTERFLOW_API_ENDPOINT").ok();
    let auth_token = std::env::var("BUTTERFLOW_API_AUTH_TOKEN").ok();

    if let (Some(api_endpoint), Some(auth_token)) = (api_endpoint.as_ref(), auth_token.as_ref()) {
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

        let pr_url = response
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|body| body.get("url").and_then(|v| v.as_str()).map(String::from));

        match &pr_url {
            Some(url) => info!("Pull request created: {}", url),
            None => info!("Pull request created successfully!"),
        }

        return Ok(pr_url);
    }

    if api_endpoint.is_some() || auth_token.is_some() {
        return Err(butterflow_models::Error::Runtime(
            "BUTTERFLOW_API_ENDPOINT and BUTTERFLOW_API_AUTH_TOKEN must either both be set or both be unset".to_string(),
        ));
    }

    create_pull_request_via_gh(title, body, draft, head, &resolved_base, working_dir).await
}

async fn create_pull_request_via_gh(
    title: &str,
    body: Option<&str>,
    draft: bool,
    head: &str,
    base: &str,
    working_dir: &std::path::Path,
) -> Result<Option<String>> {
    info!("Creating pull request via gh CLI...");

    let mut command = Command::new("gh");
    command
        .args([
            "pr",
            "create",
            "--head",
            head,
            "--base",
            base,
            "--title",
            title,
            "--body",
            body.unwrap_or(""),
        ])
        .current_dir(working_dir);
    configure_non_interactive_github_command(&mut command);

    if draft {
        command.arg("--draft");
    }

    let output = command.output().await.map_err(|e| {
        butterflow_models::Error::Runtime(format!("gh pr create failed to start: {e}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let details = normalize_gh_pr_create_error(&stderr, &stdout);
        return Err(butterflow_models::Error::Runtime(format!(
            "gh pr create failed: {details}"
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let pr_url = stdout
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with("http"))
        .map(|line| line.trim().to_string());

    match &pr_url {
        Some(url) => info!("Pull request created: {}", url),
        None => info!("Pull request created successfully via gh"),
    }

    Ok(pr_url)
}

#[cfg(test)]
mod tests {
    use super::{
        normalize_gh_pr_create_error, normalize_git_push_error, redact_git_credentials,
        sanitize_path_component, worktree_path,
    };
    use std::path::Path;

    #[test]
    fn sanitize_path_component_rewrites_dot_segments() {
        assert_eq!(sanitize_path_component("."), "worktree");
        assert_eq!(sanitize_path_component(".."), "worktree");
        assert_eq!(sanitize_path_component("...hidden"), "hidden");
        assert_eq!(sanitize_path_component("../branch"), "-branch");
    }

    #[test]
    fn worktree_path_stays_within_container_for_dot_segments() {
        let repo_root = Path::new("/tmp/example-repo");
        let worktree = worktree_path(repo_root, "..", ".");
        assert_eq!(
            worktree,
            Path::new("/tmp/example-repo.codemod-worktrees/worktree-worktree")
        );
    }

    #[test]
    fn redact_git_credentials_removes_url_userinfo() {
        assert_eq!(
            redact_git_credentials("https://token@example.com/org/repo.git"),
            "https://<redacted>@example.com/org/repo.git"
        );
        assert_eq!(
            redact_git_credentials(
                "fatal: could not read from https://user:secret@example.com/org/repo.git"
            ),
            "fatal: could not read from https://<redacted>@example.com/org/repo.git"
        );
        assert_eq!(
            redact_git_credentials("git@github.com:org/repo.git"),
            "git@github.com:org/repo.git"
        );
    }

    #[test]
    fn normalize_git_push_error_rewrites_github_auth_prompt() {
        let message = normalize_git_push_error("Username for 'https://github.com': ");
        assert!(message.contains("GitHub authentication is required to push the branch"));
        assert!(message.contains("gh auth login"));
    }

    #[test]
    fn normalize_gh_pr_create_error_rewrites_auth_failure() {
        let message = normalize_gh_pr_create_error(
            "To get started with GitHub CLI, please run:  gh auth login",
            "",
        );
        assert!(message.contains("GitHub authentication is required to create the pull request"));
        assert!(message.contains("gh auth login"));
    }
}

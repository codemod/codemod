use anyhow::Result;
use butterflow_core::config::{
    DirtyGitApprovalCallback, DirtyGitApprovalKind, DirtyGitApprovalRequest,
};
use inquire::Confirm;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

type GitDirtyCheckCallback =
    Arc<Box<dyn Fn(&Path, bool, Option<&DirtyGitApprovalCallback>) -> Result<()> + Send + Sync>>;

fn dirty_prompt(path: &Path) -> String {
    format!(
        "You have uncommitted changes in {}. Do you want to proceed anyway?",
        path.display()
    )
}

fn not_tracked_prompt(path: &Path) -> String {
    format!(
        "The target path '{}' is not tracked by Git. Do you want to proceed anyway?",
        path.display()
    )
}

fn rejection_error(request: &DirtyGitApprovalRequest) -> anyhow::Error {
    match request.kind {
        DirtyGitApprovalKind::UncommittedChanges => anyhow::anyhow!(
            "You have uncommitted changes in {}. Use --allow-dirty to proceed anyway.",
            request.path.display()
        ),
        DirtyGitApprovalKind::NotTracked => anyhow::anyhow!(
            "The target path '{}' is not tracked by Git. Use --allow-dirty to proceed anyway.",
            request.path.display()
        ),
    }
}

fn confirm_request(
    request: DirtyGitApprovalRequest,
    no_interactive: bool,
    approval_callback: Option<&DirtyGitApprovalCallback>,
) -> Result<()> {
    if let Some(callback) = approval_callback {
        if callback(&request)? {
            return Ok(());
        }
        return Err(rejection_error(&request));
    }

    if !no_interactive {
        let prompt = match request.kind {
            DirtyGitApprovalKind::UncommittedChanges => dirty_prompt(&request.path),
            DirtyGitApprovalKind::NotTracked => not_tracked_prompt(&request.path),
        };
        let proceed = Confirm::new(&prompt)
            .with_default(false)
            .prompt()
            .map_err(|error| anyhow::anyhow!("Failed to get user input: {error}"))?;
        if proceed {
            return Ok(());
        }
    }

    Err(rejection_error(&request))
}

/// Creates a callback that checks if a git repository has uncommitted changes.
///
/// When `no_interactive` is false and the repo is dirty, prompts the user to proceed.
/// When `no_interactive` is true and the repo is dirty, exits with an error.
/// The callback skips the check entirely if `allow_dirty` is true.
pub fn dirty_check(no_interactive: bool) -> GitDirtyCheckCallback {
    let checked_paths = Arc::new(Mutex::new(Vec::new()));

    Arc::new(Box::new(
        move |path: &Path,
              allow_dirty: bool,
              approval_callback: Option<&DirtyGitApprovalCallback>|
              -> Result<()> {
            // Skip if already checked or dirty is allowed
            if allow_dirty {
                return Ok(());
            }

            let path_buf = path.to_path_buf();
            if checked_paths.lock().unwrap().contains(&path_buf) {
                return Ok(());
            }

            // Check if git is available
            if Command::new("git").arg("--version").output().is_err() {
                return Ok(());
            }

            let output = Command::new("git")
                .args(["rev-parse", "--is-inside-work-tree"])
                .current_dir(path)
                .output();

            match output {
                Ok(ref out) if out.status.success() => {
                    let is_inside_work_tree = String::from_utf8_lossy(&out.stdout)
                        .trim()
                        .eq_ignore_ascii_case("true");

                    if !is_inside_work_tree {
                        confirm_request(
                            DirtyGitApprovalRequest {
                                path: path_buf.clone(),
                                kind: DirtyGitApprovalKind::NotTracked,
                            },
                            no_interactive,
                            approval_callback,
                        )?;
                        checked_paths.lock().unwrap().push(path_buf);
                        return Ok(());
                    }

                    // Check for uncommitted changes
                    let status_output = Command::new("git")
                        .args(["status", "--porcelain"])
                        .current_dir(path)
                        .output()
                        .map_err(|error| anyhow::anyhow!("Failed to run git status: {error}"))?;

                    if !status_output.stdout.is_empty() {
                        confirm_request(
                            DirtyGitApprovalRequest {
                                path: path_buf.clone(),
                                kind: DirtyGitApprovalKind::UncommittedChanges,
                            },
                            no_interactive,
                            approval_callback,
                        )?;
                        checked_paths.lock().unwrap().push(path_buf);
                        return Ok(());
                    }
                    checked_paths.lock().unwrap().push(path_buf);
                    Ok(())
                }
                _ => {
                    confirm_request(
                        DirtyGitApprovalRequest {
                            path: path_buf.clone(),
                            kind: DirtyGitApprovalKind::NotTracked,
                        },
                        no_interactive,
                        approval_callback,
                    )?;
                    checked_paths.lock().unwrap().push(path_buf);
                    Ok(())
                }
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::dirty_check;
    use butterflow_core::config::{
        DirtyGitApprovalCallback, DirtyGitApprovalKind, DirtyGitApprovalRequest,
    };
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn rejected_dirty_git_approval_does_not_cache_path() {
        let callback = dirty_check(true);
        let approvals = Arc::new(AtomicUsize::new(0));
        let approvals_for_callback = Arc::clone(&approvals);
        let approval: DirtyGitApprovalCallback =
            Arc::new(move |_request: &DirtyGitApprovalRequest| {
                approvals_for_callback.fetch_add(1, Ordering::Relaxed);
                Ok(false)
            });

        let path = Path::new("/definitely/not/a/git/repo");

        assert!(callback(path, false, Some(&approval)).is_err());
        assert!(callback(path, false, Some(&approval)).is_err());
        assert_eq!(approvals.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn approved_dirty_git_check_caches_path_after_success() {
        let callback = dirty_check(true);
        let approvals = Arc::new(AtomicUsize::new(0));
        let approvals_for_callback = Arc::clone(&approvals);
        let approval: DirtyGitApprovalCallback =
            Arc::new(move |request: &DirtyGitApprovalRequest| {
                approvals_for_callback.fetch_add(1, Ordering::Relaxed);
                assert_eq!(request.kind, DirtyGitApprovalKind::NotTracked);
                Ok(true)
            });

        let path = Path::new("/definitely/not/a/git/repo");

        assert!(callback(path, false, Some(&approval)).is_ok());
        assert!(callback(path, false, Some(&approval)).is_ok());
        assert_eq!(approvals.load(Ordering::Relaxed), 1);
    }
}

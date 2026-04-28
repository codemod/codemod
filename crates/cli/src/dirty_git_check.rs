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

            let mut paths = checked_paths.lock().unwrap();
            if paths.contains(&path.to_path_buf()) {
                return Ok(());
            }
            paths.push(path.to_path_buf());

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
                        return confirm_request(
                            DirtyGitApprovalRequest {
                                path: path.to_path_buf(),
                                kind: DirtyGitApprovalKind::NotTracked,
                            },
                            no_interactive,
                            approval_callback,
                        );
                    }

                    // Check for uncommitted changes
                    let status_output = Command::new("git")
                        .args(["status", "--porcelain"])
                        .current_dir(path)
                        .output()
                        .map_err(|error| anyhow::anyhow!("Failed to run git status: {error}"))?;

                    if !status_output.stdout.is_empty() {
                        return confirm_request(
                            DirtyGitApprovalRequest {
                                path: path.to_path_buf(),
                                kind: DirtyGitApprovalKind::UncommittedChanges,
                            },
                            no_interactive,
                            approval_callback,
                        );
                    }
                    Ok(())
                }
                _ => confirm_request(
                    DirtyGitApprovalRequest {
                        path: path.to_path_buf(),
                        kind: DirtyGitApprovalKind::NotTracked,
                    },
                    no_interactive,
                    approval_callback,
                ),
            }
        },
    ))
}

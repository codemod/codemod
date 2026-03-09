use inquire::Confirm;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

type GitDirtyCheckCallback = Arc<Box<dyn Fn(&Path, bool) + Send + Sync>>;

/// Prompts the user or exits when uncommitted changes are detected.
fn handle_dirty(path: &Path, no_interactive: bool) {
    if !no_interactive {
        let proceed = Confirm::new(&format!(
            "You have uncommitted changes in {}. Do you want to proceed anyway?",
            path.display()
        ))
        .with_default(false)
        .prompt()
        .unwrap_or(false);

        if proceed {
            return;
        }
    }
    eprintln!(
        "Error: You have uncommitted changes in {}. Use --allow-dirty to proceed anyway.",
        path.display()
    );
    std::process::exit(1);
}

/// Prompts the user or exits when the path is not tracked by Git.
fn handle_not_tracked(path: &Path, no_interactive: bool) {
    if !no_interactive {
        let proceed = Confirm::new(&format!(
            "The target path '{}' is not tracked by Git. Do you want to proceed anyway?",
            path.display()
        ))
        .with_default(false)
        .prompt()
        .unwrap_or(false);

        if proceed {
            return;
        }
    }
    eprintln!(
        "Error: The target path '{}' is not tracked by Git. Use --allow-dirty to proceed anyway.",
        path.display()
    );
    std::process::exit(1);
}

/// Creates a callback that checks if a git repository has uncommitted changes.
///
/// When `no_interactive` is false and the repo is dirty, prompts the user to proceed.
/// When `no_interactive` is true and the repo is dirty, exits with an error.
/// The callback skips the check entirely if `allow_dirty` is true.
pub fn dirty_check(no_interactive: bool) -> GitDirtyCheckCallback {
    let checked_paths = Arc::new(Mutex::new(Vec::new()));

    Arc::new(Box::new(move |path: &Path, allow_dirty: bool| {
        // Skip if already checked or dirty is allowed
        if allow_dirty {
            return;
        }

        let mut paths = checked_paths.lock().unwrap();
        if paths.contains(&path.to_path_buf()) {
            return;
        }
        paths.push(path.to_path_buf());

        // Check if git is available
        if Command::new("git").arg("--version").output().is_err() {
            return;
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
                    handle_not_tracked(path, no_interactive);
                    return;
                }

                // Check for uncommitted changes
                let status_output = Command::new("git")
                    .args(["status", "--porcelain"])
                    .current_dir(path)
                    .output()
                    .expect("Failed to run git status");

                if !status_output.stdout.is_empty() {
                    handle_dirty(path, no_interactive);
                }
            }
            _ => handle_not_tracked(path, no_interactive),
        }
    }))
}

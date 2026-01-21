use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

type GitDirtyCheckCallback = Arc<Box<dyn Fn(&Path, bool) + Send + Sync>>;

/// Prints an error message about the path not being tracked by Git and exits.
fn exit_not_tracked(path: &Path) -> ! {
    eprintln!(
        "Error: The target path '{}' is not tracked by Git. Use --allow-dirty to proceed anyway.",
        path.display()
    );
    std::process::exit(1)
}

/// Creates a callback that checks if a git repository has uncommitted changes.
///
/// The callback will exit with an error if:
/// - The path has uncommitted changes and `allow_dirty` is false
/// - The path is not tracked by git and `allow_dirty` is false
pub fn dirty_check() -> GitDirtyCheckCallback {
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
                    exit_not_tracked(path);
                }

                // Check for uncommitted changes
                let status_output = Command::new("git")
                    .args(["status", "--porcelain"])
                    .current_dir(path)
                    .output()
                    .expect("Failed to run git status");

                if !status_output.stdout.is_empty() {
                    eprintln!(
                        "Error: You have uncommitted changes in {}. Use --allow-dirty to proceed anyway.",
                        path.display()
                    );
                    std::process::exit(1);
                }
            }
            _ => exit_not_tracked(path),
        }
    }))
}

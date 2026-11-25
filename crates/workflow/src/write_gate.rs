//! The check for uncommitted changes, made before a run first changes its
//! target instead of before it starts, so a run that only reads (a check, a
//! report) never stops at it.
//!
//! What counts as changing the target: a step committing files (a
//! `forEachFile`, function, `dependencyGraph` or `astGrep` step that wrote,
//! renamed or deleted something; nested codemods' steps too), a `shell`,
//! `agent`, `installSkill` or git step, and any step whose code may use `fs`
//! or `child_process`. Dry runs, tests and resumed runs are not checked.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use crate::interaction::{Approval, Ask, Decision, Interaction};

/// Asks once per run; every later write gets the same answer.
pub(crate) struct WriteGate {
    target: PathBuf,
    answer: Mutex<Option<Result<(), String>>>,
}

/// The target's git state, as far as the check goes.
#[derive(Debug, PartialEq)]
enum TreeState {
    Clean,
    Dirty,
    NotARepository,
}

impl WriteGate {
    pub(crate) fn new(target: &Path) -> Self {
        Self {
            target: target.to_path_buf(),
            answer: Mutex::new(None),
        }
    }

    /// `Ok` when the run may change the target: it has no uncommitted
    /// changes, or the person said yes. Held while asking, so steps writing
    /// at the same time wait for the one answer.
    pub(crate) fn check(
        &self,
        interaction: &dyn Interaction,
        stop: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<(), String> {
        let mut answer = self.answer.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(answer) = answer.as_ref() {
            return answer.clone();
        }
        let path = self.target.display().to_string();
        let result = match tree_state(&self.target) {
            None | Some(TreeState::Clean) => Ok(()),
            Some(state) => {
                let not_a_repository = state == TreeState::NotARepository;
                let approval = Approval::UncommittedChanges {
                    path: path.clone(),
                    not_a_repository,
                };
                let ask = Ask {
                    id: approval.question_id(None),
                    step: None,
                    stop: Some(std::sync::Arc::clone(stop)),
                };
                match interaction.ask(&ask, &approval) {
                    Decision::Approved => Ok(()),
                    Decision::Denied | Decision::Deferred if not_a_repository => Err(format!(
                        "The target path '{path}' is not tracked by Git, and this step would change it; nothing was written. Use --allow-dirty to proceed anyway."
                    )),
                    Decision::Denied | Decision::Deferred => Err(format!(
                        "You have uncommitted changes in {path}, and this step would change files there; nothing was written. Commit or stash them, or use --allow-dirty to proceed anyway."
                    )),
                }
            }
        };
        *answer = Some(result.clone());
        result
    }
}

/// `None` when git is not installed: there is nothing to check with.
fn tree_state(target: &Path) -> Option<TreeState> {
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(target).output();
    if Command::new("git").arg("--version").output().is_err() {
        return None;
    }
    let inside = git(&["rev-parse", "--is-inside-work-tree"])
        .ok()
        .filter(|out| {
            out.status.success()
                && String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .eq_ignore_ascii_case("true")
        });
    if inside.is_none() {
        return Some(TreeState::NotARepository);
    }
    Some(match git(&["status", "--porcelain"]) {
        Ok(out) if out.status.success() && out.stdout.is_empty() => TreeState::Clean,
        _ => TreeState::Dirty,
    })
}

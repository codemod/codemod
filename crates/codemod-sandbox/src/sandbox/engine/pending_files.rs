//! Files a step has changed but not written yet.
//!
//! A step's transforms run on several threads and nothing is written until
//! the step commits, so two transforms that both change one file with
//! `jssgTransform` would each start from the text on disk and one change
//! would be lost. Reading and updating such a file through [`PendingFiles`]
//! gives it to one transform at a time and hands each the text the step has
//! so far.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::ThreadId;
use std::time::Duration;

/// How long a transform waits for a file another transform holds. Only a
/// transform that holds one file while asking for another can make a wait
/// that long.
const WAIT_LIMIT: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Pending {
    original: String,
    text: String,
    /// The thread whose transform has the file now.
    holder: Option<ThreadId>,
}

#[derive(Debug, Clone, Default)]
pub struct PendingFiles {
    inner: Arc<(Mutex<HashMap<PathBuf, Pending>>, Condvar)>,
}

/// One file taken with [`PendingFiles::take`]; gives it back when dropped.
#[derive(Debug)]
pub struct TakenFile {
    files: PendingFiles,
    path: PathBuf,
}

impl PendingFiles {
    fn lock(&self) -> MutexGuard<'_, HashMap<PathBuf, Pending>> {
        self.inner.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The file's text as the step has it so far, and the file itself until
    /// the returned guard is dropped. Waits while another transform has it.
    pub fn take(&self, path: &Path) -> std::io::Result<(String, TakenFile)> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let me = std::thread::current().id();
        let mut files = self.lock();
        if files.get(&key).is_some_and(|file| file.holder == Some(me)) {
            return Err(std::io::Error::other(format!(
                "'{}' is being transformed already; return its new text from that transform instead",
                path.display()
            )));
        }
        let waited = self
            .inner
            .1
            .wait_timeout_while(files, WAIT_LIMIT, |files| {
                files.get(&key).is_some_and(|file| file.holder.is_some())
            })
            .unwrap_or_else(|e| e.into_inner());
        if waited.1.timed_out() {
            return Err(std::io::Error::other(format!(
                "timed out waiting for another transform of '{}'",
                path.display()
            )));
        }
        files = waited.0;
        let text = match files.get_mut(&key) {
            Some(file) => {
                file.holder = Some(me);
                file.text.clone()
            }
            None => {
                let text = std::fs::read_to_string(&key)?;
                files.insert(
                    key.clone(),
                    Pending {
                        original: text.clone(),
                        text: text.clone(),
                        holder: Some(me),
                    },
                );
                text
            }
        };
        Ok((
            text,
            TakenFile {
                files: self.clone(),
                path: key,
            },
        ))
    }

    /// Every file whose text the step changed: `(path, on disk, now)`, by path.
    pub fn changed(&self) -> Vec<(PathBuf, String, String)> {
        let mut changed: Vec<_> = self
            .lock()
            .iter()
            .filter(|(_, file)| file.text != file.original)
            .map(|(path, file)| (path.clone(), file.original.clone(), file.text.clone()))
            .collect();
        changed.sort();
        changed
    }
}

impl TakenFile {
    /// The path the file is kept under.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file's new text, for whoever takes it next and for the commit.
    pub fn set(&self, text: String) {
        if let Some(file) = self.files.lock().get_mut(&self.path) {
            file.text = text;
        }
    }
}

impl Drop for TakenFile {
    fn drop(&mut self) {
        if let Some(file) = self.files.lock().get_mut(&self.path) {
            file.holder = None;
        }
        self.files.inner.1.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_to_one_file_from_several_threads_add_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.ts");
        std::fs::write(&path, "").unwrap();
        let files = PendingFiles::default();
        std::thread::scope(|scope| {
            for line in 0..8 {
                let (files, path) = (files.clone(), path.clone());
                scope.spawn(move || {
                    let (text, taken) = files.take(&path).unwrap();
                    std::thread::yield_now();
                    taken.set(format!("{text}export * from './{line}';\n"));
                });
            }
        });
        let changed = files.changed();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].1, "");
        assert_eq!(changed[0].2.lines().count(), 8);
        // Nothing reached the disk.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    }

    #[test]
    fn taking_a_file_twice_on_one_thread_fails_instead_of_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.ts");
        std::fs::write(&path, "x").unwrap();
        let files = PendingFiles::default();
        let (_, _taken) = files.take(&path).unwrap();
        let error = files.take(&path).unwrap_err().to_string();
        assert!(error.contains("being transformed already"), "{error}");
    }

    #[test]
    fn a_file_left_as_it_was_is_not_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.ts");
        std::fs::write(&path, "x").unwrap();
        let files = PendingFiles::default();
        drop(files.take(&path).unwrap());
        assert!(files.changed().is_empty());
    }
}

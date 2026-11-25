//! The file texts a provider answers about during one step.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// File texts as they were when the step started. A file is pinned the first
/// time a query reads it, or when the run tells the provider its text (an
/// earlier step's edit in a dry run). Later reads return the pinned text, so
/// every answer in a step describes the same workspace, whatever the step
/// writes to disk meanwhile.
#[derive(Debug, Default)]
pub struct FileSnapshot {
    files: RwLock<HashMap<PathBuf, Arc<str>>>,
}

impl FileSnapshot {
    pub fn new() -> Self {
        Self::default()
    }

    /// The pinned text of `path`, reading and pinning it with `read` first.
    pub fn get_or_read<E>(
        &self,
        path: &Path,
        read: impl FnOnce() -> Result<String, E>,
    ) -> Result<Arc<str>, E> {
        let key = key(path);
        if let Some(text) = self
            .files
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Ok(Arc::clone(text));
        }
        let text: Arc<str> = read()?.into();
        let mut files = self.files.write().unwrap_or_else(|e| e.into_inner());
        Ok(Arc::clone(files.entry(key).or_insert(text)))
    }

    /// Pin `path` to `content`.
    pub fn pin(&self, path: &Path, content: &str) {
        self.files
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key(path), content.into());
    }

    /// Forget every pinned text.
    pub fn clear(&self) {
        self.files
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

fn key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_file_keeps_its_first_text() {
        let snapshot = FileSnapshot::new();
        let path = Path::new("/nowhere/a.ts");
        let first = snapshot.get_or_read(path, || Ok::<_, ()>("old".into()));
        let second = snapshot.get_or_read(path, || Ok::<_, ()>("new".into()));
        assert_eq!((&*first.unwrap(), &*second.unwrap()), ("old", "old"));
    }
}

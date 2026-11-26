//! A run's target when it is not a directory on disk: a read-only snapshot
//! a host keeps elsewhere (Grepkon serves repository snapshots from its own
//! storage). Such a run is always a dry run: steps read the snapshot through
//! the run's overlay and their changes stay there.

use std::sync::Arc;

/// The files of a read-only snapshot.
pub trait TargetFiles: Send + Sync {
    /// Every file in the snapshot, relative, with `/`, sorted. What the host
    /// does not serve (binary or oversized files, ignored paths) is left out:
    /// steps never see it.
    fn paths(&self) -> &[String];

    /// The text of each of `paths`, in order: `None` for a file the snapshot
    /// does not have or cannot read as text. Called with a batch at a time,
    /// so a host can fetch them together.
    fn read(&self, paths: &[&str]) -> Vec<Option<Arc<str>>>;

    /// A step is about to read `paths`, a file at a time: a host that fetches
    /// in batches can fetch them now. Nothing by default.
    fn prefetch(&self, _paths: &[&str]) {}

    /// Whether the snapshot has `path` as a file.
    fn is_file(&self, path: &str) -> bool {
        self.paths()
            .binary_search_by(|probe| probe.as_str().cmp(path))
            .is_ok()
    }

    /// Whether the snapshot has files under `path`.
    fn is_dir(&self, path: &str) -> bool {
        let prefix = format!("{path}/");
        let paths = self.paths();
        let start = paths.partition_point(|probe| probe.as_str() < prefix.as_str());
        paths.get(start).is_some_and(|p| p.starts_with(&prefix))
    }
}

/// A snapshot held in memory, for tests and small hosts.
pub struct MemoryTarget {
    paths: Vec<String>,
    texts: std::collections::BTreeMap<String, Arc<str>>,
}

impl MemoryTarget {
    pub fn new(files: impl IntoIterator<Item = (String, String)>) -> Self {
        let texts: std::collections::BTreeMap<String, Arc<str>> = files
            .into_iter()
            .map(|(path, text)| (path, Arc::from(text)))
            .collect();
        Self {
            paths: texts.keys().cloned().collect(),
            texts,
        }
    }
}

impl TargetFiles for MemoryTarget {
    fn paths(&self) -> &[String] {
        &self.paths
    }

    fn read(&self, paths: &[&str]) -> Vec<Option<Arc<str>>> {
        paths
            .iter()
            .map(|path| self.texts.get(*path).cloned())
            .collect()
    }
}

/// Remembers what `forEachFile` callbacks did to files, across runs: a file
/// whose key was seen is not run again. The key covers the callback's built
/// module, its captures, the step's language, semantic analysis and
/// capabilities, and the file's path and text; the host's namespace covers
/// the rest (the package version, whose helper files a callback imports).
/// Outcomes that failed, used `assess()`, or ran with `fetch` are not kept.
pub trait FileCache: Send + Sync {
    fn get(&self, key: &[u8; 32]) -> Option<Arc<str>>;
    fn put(&self, key: [u8; 32], outcome: String);
    /// Mixed into every key: what else the outcome depends on.
    fn namespace(&self) -> &[u8] {
        &[]
    }
}

/// Told the output of each file a `forEachFile` step's callback returned
/// one for, with the step's position and label, as the step finishes.
pub trait FileOutputs: Send + Sync {
    fn output(&self, position: u64, label: &str, path: &str, output: &serde_json::Value);
}

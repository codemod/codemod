//! A `vfs::FileSystem` backend that wraps an in-memory store with a lazy
//! [`FileFetcher`] read-through.

use std::io::Write;
use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use vfs::error::VfsErrorKind;
use vfs::{FileSystem, MemoryFS, SeekAndRead, SeekAndWrite, VfsError, VfsMetadata, VfsResult};

use crate::sandbox::engine::curated_fs::FileFetcher;

/// bookkeeping.
#[derive(Clone)]
pub struct FetchingMemoryFs {
    /// Owned in-memory store. All actual storage lives here; the wrapper
    /// only adds lazy read-through bookkeeping on top. Held via `Arc`
    /// because `MemoryFS` is not itself `Clone`, and we need to share
    /// one store between the seeding handle and the `VfsPath` wrapper.
    inner: Arc<MemoryFS>,
    /// Upstream file source consulted on the first read of a stub path.
    /// Called with the repo-relative form of the path (sandbox prefix
    /// stripped) because the fetcher speaks the upstream repository's
    /// address space, not the sandboxed one.
    fetcher: Arc<dyn FileFetcher>,
    /// Absolute sandbox root (e.g. `/app`). Paths whose absolute form
    /// equals this root or begins with `<root>/` have the prefix
    /// stripped when forwarded to the fetcher. Paths outside are never
    /// forwarded (the fetcher only knows about repo paths) — this is
    /// the sandbox boundary.
    sandbox_root: Arc<str>,
    /// Single-map design (vs. separate `stubs` + `hydrated` DashMaps)
    /// closes the thundering-herd race: two readers observing the same
    /// `Stub` entry share one `Arc<OnceLock>` so the fetcher runs at
    /// most once per path regardless of whether the underlying
    /// `FileFetcher` itself dedups.
    state: Arc<DashMap<String, PathState>>,
}

#[derive(Clone)]
enum PathState {
    Stub(Arc<OnceLock<Result<(), String>>>),
    Hydrated,
}

impl std::fmt::Debug for FetchingMemoryFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchingMemoryFs")
            .field("sandbox_root", &self.sandbox_root.as_ref())
            .field("tracked_paths", &self.state.len())
            .finish()
    }
}

impl FetchingMemoryFs {
    pub fn new(fetcher: Arc<dyn FileFetcher>, sandbox_root: impl Into<String>) -> Self {
        let sandbox_root: String = sandbox_root.into();
        Self {
            inner: Arc::new(MemoryFS::new()),
            fetcher,
            sandbox_root: Arc::from(sandbox_root),
            state: Arc::new(DashMap::new()),
        }
    }

    /// Record that `path` exists in the upstream store without fetching
    /// it. `path` must be absolute (vfs-style: starts with `/`).
    /// Relative paths would create inconsistent state between
    /// `ensure_parents` (which walks from root) and the inner
    /// `create_file` (which would insert the relative form verbatim).
    /// In debug builds this is a hard assert; in release we early-
    /// return so a stray caller can't desync the map.
    ///
    /// Creates an empty file + parent directories in the inner memfs so
    /// `read_dir`, `exists`, and `metadata` all behave as if the file
    /// were present. On the first `open_file(path)`, the fetcher is
    /// consulted and the inner memfs is overwritten with real content.
    pub fn stub_path(&self, path: &str) {
        if path.is_empty() {
            return;
        }
        debug_assert!(
            path.starts_with('/'),
            "FetchingMemoryFs::stub_path requires an absolute path, got {path:?}"
        );
        if !path.starts_with('/') {
            return;
        }
        // Existing `Hydrated` wins: the caller already has authoritative
        // content, so registering a stub on top would wrongly re-enable
        // fetcher traffic.
        if matches!(
            self.state.get(path).map(|v| v.clone()),
            Some(PathState::Hydrated)
        ) {
            return;
        }
        ensure_parents(&self.inner, path);
        // It's fine if the file already exists — seeding the same stub
        // twice is idempotent and non-fatal in all the paths that hit
        // this code.
        let _ = self.inner.create_file(path);
        self.state
            .entry(path.to_string())
            .or_insert_with(|| PathState::Stub(Arc::new(OnceLock::new())));
    }

    /// Write `content` into `path` authoritatively (the caller has the
    /// final bytes) and mark the path hydrated. Future reads skip the
    /// fetcher.
    pub fn write_authoritative(&self, path: &str, content: &[u8]) -> VfsResult<()> {
        ensure_parents(&self.inner, path);
        let mut writer = self.inner.create_file(path)?;
        writer
            .write_all(content)
            .map_err(|e| VfsError::from(VfsErrorKind::IoError(e)))?;
        drop(writer);
        self.mark_hydrated(path);
        Ok(())
    }

    fn mark_hydrated(&self, path: &str) {
        self.state.insert(path.to_string(), PathState::Hydrated);
    }

    /// Translate a vfs-absolute path to the repo-relative form the
    /// fetcher expects. Returns `None` if the path is outside the
    /// sandbox (in which case the fetcher is not consulted).
    fn to_repo_path(&self, path: &str) -> Option<String> {
        let prefix = self.sandbox_root.as_ref().trim_end_matches('/');
        if prefix.is_empty() {
            return Some(path.trim_start_matches('/').to_string());
        }
        if path == prefix {
            return Some(String::new());
        }
        let child_prefix = format!("{prefix}/");
        let stripped = path.strip_prefix(&child_prefix)?;
        Some(stripped.to_string())
    }

    fn run_fetch(&self, path: &str) -> Result<(), String> {
        let repo_path = match self.to_repo_path(path) {
            Some(p) => p,
            None => return Ok(()), // outside sandbox → pass through
        };
        match self.fetcher.fetch(&repo_path) {
            Ok(Some(bytes)) => {
                ensure_parents(&self.inner, path);
                let mut writer = self.inner.create_file(path).map_err(|e| e.to_string())?;
                writer.write_all(&bytes).map_err(|e| e.to_string())?;
                drop(writer);
                Ok(())
            }
            Ok(None) => Err(FILE_NOT_FOUND.to_string()),
            Err(msg) => Err(format!("fetcher failed for {path}: {msg}")),
        }
    }
}

const FILE_NOT_FOUND: &str = "__fetching_vfs_file_not_found__";

/// Walk a `/`-separated absolute path and make sure every parent
/// directory exists in `fs`. Mirrors `mkdir -p`. Failures on individual
/// components are non-fatal here: stubs are a best-effort seeding step
/// and a pre-existing directory at the same path satisfies our invariant
/// regardless of the error.
fn ensure_parents(fs: &MemoryFS, path: &str) {
    let Some(last_slash) = path.rfind('/') else {
        return;
    };
    let parent = &path[..last_slash];
    if parent.is_empty() {
        return;
    }
    // Create each intermediate directory in order. The MemoryFS requires
    // the parent of a new directory to exist, so iterate top-down.
    let mut cursor = String::new();
    for component in parent.trim_start_matches('/').split('/') {
        if component.is_empty() {
            continue;
        }
        cursor.push('/');
        cursor.push_str(component);
        if !fs.exists(&cursor).unwrap_or(false) {
            let _ = fs.create_dir(&cursor);
        }
    }
}

impl FileSystem for FetchingMemoryFs {
    fn read_dir(&self, path: &str) -> VfsResult<Box<dyn Iterator<Item = String> + Send>> {
        self.inner.read_dir(path)
    }

    fn create_dir(&self, path: &str) -> VfsResult<()> {
        self.inner.create_dir(path)
    }

    fn open_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndRead + Send>> {
        // Pull the current state snapshot (cloning the `Arc<OnceLock>`
        // release the DashMap shard guard before we call into the
        // fetcher. It avoids deadlocks if the fetcher re-enters VFS).
        let snapshot = self.state.get(path).map(|v| v.clone());
        match snapshot {
            Some(PathState::Stub(cell)) => {
                let outcome = cell.get_or_init(|| self.run_fetch(path)).clone();
                match outcome {
                    Ok(()) => {
                        self.state.insert(path.to_string(), PathState::Hydrated);
                    }
                    Err(ref msg) if msg == FILE_NOT_FOUND => {
                        self.state.insert(path.to_string(), PathState::Hydrated);
                        return Err(VfsError::from(VfsErrorKind::FileNotFound));
                    }
                    Err(msg) => {
                        return Err(VfsError::from(VfsErrorKind::Other(msg.clone())));
                    }
                }
            }
            Some(PathState::Hydrated) | None => {}
        }
        self.inner.open_file(path)
    }

    fn create_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndWrite + Send>> {
        let writer = self.inner.create_file(path)?;
        self.mark_hydrated(path);
        Ok(writer)
    }

    fn append_file(&self, path: &str) -> VfsResult<Box<dyn SeekAndWrite + Send>> {
        let writer = self.inner.append_file(path)?;
        self.mark_hydrated(path);
        Ok(writer)
    }

    fn metadata(&self, path: &str) -> VfsResult<VfsMetadata> {
        self.inner.metadata(path)
    }

    fn exists(&self, path: &str) -> VfsResult<bool> {
        self.inner.exists(path)
    }

    fn remove_file(&self, path: &str) -> VfsResult<()> {
        self.state.remove(path);
        self.inner.remove_file(path)
    }

    fn remove_dir(&self, path: &str) -> VfsResult<()> {
        self.inner.remove_dir(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use vfs::VfsPath;

    struct RecordingFetcher {
        calls: Mutex<Vec<String>>,
        hits: AtomicUsize,
        responses: Mutex<std::collections::HashMap<String, Option<Vec<u8>>>>,
    }

    impl RecordingFetcher {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                hits: AtomicUsize::new(0),
                responses: Mutex::new(std::collections::HashMap::new()),
            }
        }

        fn serve(&self, repo_path: &str, bytes: &[u8]) {
            self.responses
                .lock()
                .unwrap()
                .insert(repo_path.to_string(), Some(bytes.to_vec()));
        }

        fn serve_missing(&self, repo_path: &str) {
            self.responses
                .lock()
                .unwrap()
                .insert(repo_path.to_string(), None);
        }
    }

    impl FileFetcher for RecordingFetcher {
        fn fetch(&self, path: &str) -> std::result::Result<Option<Vec<u8>>, String> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            self.calls.lock().unwrap().push(path.to_string());
            Ok(self
                .responses
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .unwrap_or(None))
        }
    }

    fn read_to_string(root: &VfsPath, path: &str) -> String {
        let vp = root.join(path.trim_start_matches('/')).unwrap();
        let mut reader = vp.open_file().unwrap();
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();
        buf
    }

    #[test]
    fn stub_read_triggers_fetch_once() {
        let fetcher = Arc::new(RecordingFetcher::new());
        fetcher.serve("src/utils.ts", b"export const answer = 42;");
        let fs = FetchingMemoryFs::new(fetcher.clone() as Arc<dyn FileFetcher>, "/app");
        fs.stub_path("/app/src/utils.ts");

        let root: VfsPath = fs.into();
        let content = read_to_string(&root, "app/src/utils.ts");
        assert_eq!(content, "export const answer = 42;");

        // A second read must NOT hit the fetcher — hydration marked the
        // path and subsequent reads are pure inner-memfs hits.
        let content_again = read_to_string(&root, "app/src/utils.ts");
        assert_eq!(content_again, "export const answer = 42;");

        assert_eq!(fetcher.hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            &*fetcher.calls.lock().unwrap(),
            &vec!["src/utils.ts".to_string()]
        );
    }

    #[test]
    fn authoritative_write_skips_fetch() {
        let fetcher = Arc::new(RecordingFetcher::new());
        let fs = FetchingMemoryFs::new(fetcher.clone() as Arc<dyn FileFetcher>, "/app");
        // Caller has the final content; never declared as stub.
        fs.write_authoritative("/app/src/batch.ts", b"export const x = 1;")
            .unwrap();

        let root: VfsPath = fs.into();
        let content = read_to_string(&root, "app/src/batch.ts");
        assert_eq!(content, "export const x = 1;");
        assert_eq!(fetcher.hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stub_then_authoritative_cancels_stub() {
        let fetcher = Arc::new(RecordingFetcher::new());
        fetcher.serve("src/foo.ts", b"should never be read");
        let fs = FetchingMemoryFs::new(fetcher.clone() as Arc<dyn FileFetcher>, "/app");
        fs.stub_path("/app/src/foo.ts");
        // Caller realized they have the content and wrote it directly.
        fs.write_authoritative("/app/src/foo.ts", b"real content")
            .unwrap();

        let root: VfsPath = fs.into();
        assert_eq!(read_to_string(&root, "app/src/foo.ts"), "real content");
        assert_eq!(fetcher.hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn fetcher_returning_none_becomes_file_not_found() {
        let fetcher = Arc::new(RecordingFetcher::new());
        fetcher.serve_missing("src/ghost.ts");
        let fs = FetchingMemoryFs::new(fetcher.clone() as Arc<dyn FileFetcher>, "/app");
        fs.stub_path("/app/src/ghost.ts");

        let root: VfsPath = fs.into();
        let vp = root.join("app/src/ghost.ts").unwrap();
        let result = vp.open_file();
        match result {
            Ok(_) => panic!("missing upstream file must error"),
            Err(err) => assert!(matches!(err.kind(), VfsErrorKind::FileNotFound)),
        }

        // Second open shouldn't re-hit the fetcher — we mark hydrated on
        // negative results too.
        let _ = vp.open_file();
        assert_eq!(fetcher.hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn paths_outside_sandbox_never_touch_fetcher() {
        let fetcher = Arc::new(RecordingFetcher::new());
        // Intentionally no `serve` calls; any fetcher hit would panic
        // via the recording setup reporting an unexpected call.
        let fs = FetchingMemoryFs::new(fetcher.clone() as Arc<dyn FileFetcher>, "/app");
        // Create a file outside the sandbox; reads should go straight
        // through to inner memfs without consulting the fetcher.
        fs.write_authoritative("/other/scratch.txt", b"outside")
            .unwrap();

        let root: VfsPath = fs.into();
        assert_eq!(read_to_string(&root, "other/scratch.txt"), "outside");
        assert_eq!(fetcher.hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn concurrent_readers_hydrate_stub_exactly_once() {
        // Two threads race to read the same stub. The FetchingMemoryFs
        // must dedup through its OnceLock-per-path so the (possibly
        // non-deduping) fetcher sees at most one call regardless of
        // how many workers observed the stub.
        use std::sync::Barrier;

        struct SlowFetcher {
            hits: AtomicUsize,
            barrier: Arc<Barrier>,
        }
        impl FileFetcher for SlowFetcher {
            fn fetch(&self, path: &str) -> std::result::Result<Option<Vec<u8>>, String> {
                self.hits.fetch_add(1, Ordering::SeqCst);
                // Park here to give the second reader time to reach
                // `open_file`; the barrier releases only when both have
                // arrived, making a doubled fetch obvious if the dedup
                // is broken.
                self.barrier.wait();
                Ok(Some(format!("// {path}").into_bytes()))
            }
        }

        let barrier = Arc::new(Barrier::new(1));
        let fetcher = Arc::new(SlowFetcher {
            hits: AtomicUsize::new(0),
            barrier: Arc::clone(&barrier),
        });
        let fs = FetchingMemoryFs::new(fetcher.clone() as Arc<dyn FileFetcher>, "/app");
        fs.stub_path("/app/src/shared.ts");

        let root: VfsPath = fs.into();
        let root_a = root.clone();
        let root_b = root.clone();

        let t1 = std::thread::spawn(move || read_to_string(&root_a, "app/src/shared.ts"));
        let t2 = std::thread::spawn(move || read_to_string(&root_b, "app/src/shared.ts"));

        let a = t1.join().unwrap();
        let b = t2.join().unwrap();
        // Fetcher was called with the repo-relative form (sandbox-stripped).
        assert_eq!(a, "// src/shared.ts");
        assert_eq!(b, "// src/shared.ts");
        assert_eq!(
            fetcher.hits.load(Ordering::SeqCst),
            1,
            "OnceLock-per-path should collapse concurrent readers into a single fetch"
        );
    }

    #[test]
    fn sibling_prefix_paths_are_not_treated_as_inside_sandbox() {
        // Without a path-segment boundary, `/app2/foo` would hit the
        // fetcher because its string starts with `/app`. This test
        // pins that the fetcher is NOT consulted for siblings — the
        // sandbox boundary must be a real path separator.
        struct RefusalFetcher;
        impl FileFetcher for RefusalFetcher {
            fn fetch(&self, path: &str) -> std::result::Result<Option<Vec<u8>>, String> {
                panic!("fetcher must not be called for out-of-sandbox path {path}");
            }
        }

        let fs = FetchingMemoryFs::new(Arc::new(RefusalFetcher) as Arc<dyn FileFetcher>, "/app");
        // Directly populate a sibling path outside the sandbox.
        fs.write_authoritative("/app2/foo.ts", b"outside").unwrap();

        let root: VfsPath = fs.into();
        assert_eq!(read_to_string(&root, "app2/foo.ts"), "outside");
    }

    #[test]
    fn create_file_error_leaves_state_untouched() {
        // If the inner memfs rejects a `create_file` (e.g. missing
        // parent directory that a stub had already materialized), the
        // wrapper must not flip state to `Hydrated` — a caller retrying
        // via `open_file` should still be able to consult the fetcher
        // for the original stub. Regression guard for a PR review note.
        let fetcher = Arc::new(RecordingFetcher::new());
        fetcher.serve("src/foo.ts", b"fetched content");
        let fs = FetchingMemoryFs::new(fetcher.clone() as Arc<dyn FileFetcher>, "/app");
        fs.stub_path("/app/src/foo.ts");

        // Attempt a create under a non-existent parent; MemoryFS will
        // reject this and `create_file` must propagate the error
        // without mutating our state map.
        let create_err = FileSystem::create_file(&fs, "/app/missing/intermediate/x.ts");
        assert!(create_err.is_err(), "expected create_file to fail");

        // Stub for /app/src/foo.ts must still resolve via the fetcher.
        let root: VfsPath = fs.into();
        assert_eq!(read_to_string(&root, "app/src/foo.ts"), "fetched content");
        assert_eq!(fetcher.hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn read_dir_sees_stubs() {
        // The VFS walker in accurate.rs relies on `read_dir` to enumerate
        // indexable files. Stubs must appear in the enumeration even
        // though their content hasn't been fetched.
        let fetcher = Arc::new(RecordingFetcher::new());
        let fs = FetchingMemoryFs::new(fetcher.clone() as Arc<dyn FileFetcher>, "/app");
        fs.stub_path("/app/src/a.ts");
        fs.stub_path("/app/src/b.ts");

        let root: VfsPath = fs.into();
        let mut names: Vec<String> = root
            .join("app/src")
            .unwrap()
            .read_dir()
            .unwrap()
            .map(|p| p.filename())
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.ts".to_string(), "b.ts".to_string()]);
        assert_eq!(fetcher.hits.load(Ordering::SeqCst), 0);
    }
}

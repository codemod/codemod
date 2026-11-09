//! The bundle cache. Entries are keyed by content hashes and validated
//! against every input file on use, so stale entries are rebuilt rather than
//! trusted. A directory that is not a real directory owned by this user, or
//! that others can write to, is never used: the cache holds code that runs.
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use serde::{Deserialize, Serialize};

use crate::{
    bundle::{build_quickjs_bundle, Runtime},
    sha256, BuildError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedBundle {
    pub path: PathBuf,
    pub hash: String,
    /// False when this call built the bundle.
    pub cached: bool,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    builder: String,
    workflow: PathBuf,
    bundle: String,
    hash: String,
    inputs: BTreeMap<PathBuf, String>,
}

/// Identity of this builder (its sources and pinned toolchain): a cached
/// bundle is reused only by the same build code.
fn builder() -> &'static str {
    static BUILDER: OnceLock<String> = OnceLock::new();
    BUILDER.get_or_init(|| {
        sha256(&[
            include_bytes!("../Cargo.toml"),
            include_bytes!("bundle.rs"),
            include_bytes!("cache.rs"),
            include_bytes!("split.rs"),
        ])
    })
}

/// `quickjs/` under `$CODEMOD_WORKFLOW_CACHE`, else under
/// `<tmpdir>/codemod-workflow-cache-<uid>`. Fails when that directory is not
/// private to this user.
pub fn default_bundle_cache() -> Result<PathBuf, String> {
    default_cache("quickjs")
}

/// Where [`crate::Runtime::embedded`] writes the runtime's sources.
pub fn default_runtime_cache() -> Result<PathBuf, String> {
    default_cache("runtime")
}

/// `<root>/<name>`, root `$CODEMOD_WORKFLOW_CACHE` or a per-user temp directory.
fn default_cache(name: &str) -> Result<PathBuf, String> {
    let root = match std::env::var_os("CODEMOD_WORKFLOW_CACHE") {
        Some(root) if !root.is_empty() => PathBuf::from(root),
        _ => std::env::temp_dir().join(format!("codemod-workflow-cache-{}", uid())),
    };
    private_directory(name, &root).ok_or_else(|| {
        format!(
            "the workflow cache {} is not a directory private to this user; fix its owner and permissions or set CODEMOD_WORKFLOW_CACHE",
            root.join(name).display()
        )
    })
}

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// `<root>/<name>`, created (mode 0700) if missing; `None` when the root or
/// the directory is not private to this user (then nothing is cached).
pub fn private_directory(name: &str, root: &Path) -> Option<PathBuf> {
    let is_private = |path: &Path| {
        fs::symlink_metadata(path)
            .is_ok_and(|stat| stat.is_dir() && stat.mode() & 0o022 == 0 && stat.uid() == uid())
    };
    let create = |path: &Path| {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
    };
    // The root is checked before anything is created inside it.
    create(root).ok()?;
    if !is_private(root) {
        return None;
    }
    let directory = root.join(name);
    create(&directory).ok()?;
    is_private(&directory).then_some(directory)
}

fn file_hash(path: &Path) -> Option<String> {
    fs::read(path).ok().map(|bytes| sha256(&[&bytes]))
}

/// Write via a temporary file and a rename, so readers never see a partial entry.
fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?
        .write_all(content.as_bytes())?;
    fs::rename(&temporary, path)
}

fn cached(manifest_path: &Path, cache: &Path, workflow: &Path) -> Option<CachedBundle> {
    let manifest: Manifest = serde_json::from_slice(&fs::read(manifest_path).ok()?).ok()?;
    let bundle = cache.join(&manifest.bundle);
    let valid = manifest.builder == builder()
        && manifest.workflow == workflow
        && file_hash(&bundle).as_deref() == Some(manifest.hash.as_str())
        && manifest
            .inputs
            .iter()
            .all(|(input, hash)| file_hash(input).as_deref() == Some(hash.as_str()));
    valid.then_some(CachedBundle {
        path: bundle,
        hash: manifest.hash,
        cached: true,
    })
}

/// The bundle for `workflow` from `cache`, rebuilt only when an input file
/// (the workflow graph, the runtime sources, bundled packages) or this
/// builder changed since it was cached. Bundles are stored by content hash.
pub async fn cached_quickjs_bundle(
    workflow: &Path,
    runtime: &Runtime,
    cache: &Path,
) -> Result<CachedBundle, BuildError> {
    let workflow = std::path::absolute(workflow).unwrap_or_else(|_| workflow.to_path_buf());
    let key = sha256(&[builder().as_bytes(), workflow.to_string_lossy().as_bytes()]);
    let manifest_path = cache.join(format!("{key}.json"));
    if let Some(hit) = cached(&manifest_path, cache, &workflow) {
        return Ok(hit);
    }
    let built = build_quickjs_bundle(&workflow, runtime).await?;
    let io_error = |error: std::io::Error| BuildError {
        file: cache.to_path_buf(),
        position: None,
        message: format!("cannot write the bundle cache: {error}"),
    };
    fs::create_dir_all(cache).map_err(io_error)?;
    let bundle_name = format!("{}.mjs", built.hash);
    let bundle_path = cache.join(&bundle_name);
    if file_hash(&bundle_path).as_deref() != Some(built.hash.as_str()) {
        atomic_write(&bundle_path, &built.code).map_err(io_error)?;
    }
    let manifest = Manifest {
        builder: builder().to_owned(),
        workflow,
        bundle: bundle_name,
        hash: built.hash.clone(),
        inputs: built
            .inputs
            .iter()
            .map(|input| (input.clone(), file_hash(input).unwrap_or_default()))
            .collect(),
    };
    let manifest = serde_json::to_string(&manifest).expect("the manifest serializes");
    atomic_write(&manifest_path, &manifest).map_err(io_error)?;
    Ok(CachedBundle {
        path: bundle_path,
        hash: built.hash,
        cached: false,
    })
}

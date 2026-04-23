//! Curated filesystem module for codemod sandboxes.
//!
//! When the caller opts the codemod into the `Fs` llrt capability explicitly,
//! the llrt fs module is used instead and this curated module is not
//! registered — see `in_memory_engine.rs` for the wiring.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rquickjs::{
    module::{Declarations, Exports, ModuleDef},
    prelude::{Async, Func, Opt},
    Ctx, Error, Exception, IntoJs, JsLifetime, Object, Result, TypedArray, Value,
};
use vfs::error::VfsErrorKind;
use vfs::{VfsError, VfsFileType, VfsPath};

/// Fallback resolver invoked when a read targets a path that is inside the
/// curated sandbox's `target_dir` but isn't present in the backing VFS.
///
/// Returning `Ok(Some(bytes))` tells the curated fs to write the bytes into
/// the VFS (so subsequent reads are pure memory hits) and hand them back to
/// the codemod. `Ok(None)` means the upstream store confirmed the file
/// genuinely doesn't exist — the codemod sees `ENOENT`. `Err(msg)` means an
/// infrastructure failure and surfaces as `EIO`.
pub trait FileFetcher: Send + Sync {
    fn fetch(&self, path: &str) -> std::result::Result<Option<Vec<u8>>, String>;
}

/// Stored in rquickjs userdata so module methods can look up the backing
/// filesystem and prefix.
#[derive(Clone)]
pub struct CuratedFsConfig {
    /// Absolute path prefix that every fs operation must stay within. Incoming
    /// paths are resolved against this and rejected with `EACCES` if their
    /// normalized form escapes.
    pub target_dir: String,
    /// Backing virtual filesystem. All reads/writes are routed through this.
    pub root: VfsPath,
    /// Optional fallback that fills VFS misses from an external store. When
    /// set, a read that resolves inside `target_dir` but isn't in `root`
    /// consults the fetcher; fetched bytes are written into `root` so
    /// subsequent reads (including from other workers sharing `root`) are
    /// pure memory hits.
    pub fetcher: Option<Arc<dyn FileFetcher>>,
    /// When set, indicates that `root` maps 1:1 to real-disk paths and gives
    /// the host-filesystem path that corresponds to [`Self::target_dir`]. The
    /// resolver uses this to additionally reject any requested path that
    /// traverses a symlink; without this extra check, a `/repo/link/secret`
    /// where `link` is a symlink to `/etc` would pass the lexical prefix
    /// guard and let the codemod read outside the sandbox.
    pub physical_target_dir: Option<PathBuf>,
}

// `CuratedFsConfig` contains no `'js`-bound references, so the `'js` lifetime
// in rquickjs userdata is a no-op for it.
unsafe impl<'js> JsLifetime<'js> for CuratedFsConfig {
    type Changed<'to> = CuratedFsConfig;
}

impl CuratedFsConfig {
    pub fn new(target_dir: impl Into<String>, root: VfsPath) -> Self {
        Self {
            target_dir: target_dir.into(),
            root,
            fetcher: None,
            physical_target_dir: None,
        }
    }

    /// Attach a fallback [`FileFetcher`] that is consulted when a read targets
    /// a path inside `target_dir` that isn't currently in the VFS.
    pub fn with_fetcher(mut self, fetcher: Arc<dyn FileFetcher>) -> Self {
        self.fetcher = Some(fetcher);
        self
    }

    /// Declare that the backing VFS maps 1:1 to real-disk paths (e.g.
    /// `PhysicalFS::new("/")`) and give the host path corresponding to
    /// `target_dir`. Enables symlink-safe resolution: any requested path
    /// whose existing intermediate components include a symlink is rejected
    /// with `EACCES`, preventing sandbox escape via crafted or pre-existing
    /// symlinks. Leave unset for in-memory VFS backends.
    pub fn with_physical_target_dir(mut self, physical_target_dir: PathBuf) -> Self {
        self.physical_target_dir = Some(physical_target_dir);
        self
    }

    fn normalized_target(&self) -> String {
        let trimmed = self.target_dir.trim_end_matches('/');
        if trimmed.is_empty() {
            "/".to_string()
        } else {
            trimmed.to_string()
        }
    }

    /// Resolve an incoming path against [`Self::target_dir`]. Relative paths
    /// are resolved beneath `target_dir`; absolute paths are left as-is but
    /// must normalize to a location inside `target_dir`. When
    /// [`Self::physical_target_dir`] is set, also rejects paths whose
    /// existing intermediate components include a symlink (so a codemod
    /// can't escape via `target_dir/link-to-outside/...`).
    fn resolve(&self, input: &str) -> std::result::Result<(String, VfsPath), FsErrorKind> {
        let target = self.normalized_target();
        let raw = if input.starts_with('/') {
            input.to_string()
        } else if target == "/" {
            format!("/{input}")
        } else {
            format!("{target}/{input}")
        };
        let normalized = normalize_path(&raw);
        let within_target = normalized == target
            || (target == "/" && normalized.starts_with('/'))
            || normalized.starts_with(&format!("{target}/"));
        if !within_target {
            return Err(FsErrorKind::AccessDenied { path: normalized });
        }
        if let Some(phys_target) = &self.physical_target_dir {
            check_no_symlink_escape(phys_target, &target, &normalized)?;
        }
        let vfs_path = self
            .root
            .join(normalized.trim_start_matches('/'))
            .map_err(|_| FsErrorKind::InvalidPath {
                path: normalized.clone(),
            })?;
        Ok((normalized, vfs_path))
    }
}

/// Walk the host-filesystem path corresponding to `normalized` starting from
/// `phys_target` and reject if any component exists and is a symlink. This
/// closes the gap that pure lexical normalization leaves open on real disk:
/// `/repo/link/secret` passes the prefix check even when `link` is a symlink
/// to `/etc`, and would otherwise let a codemod read outside the sandbox.
/// Components that don't exist yet (e.g. when writing a fresh file) are
/// ignored — no symlink can exist at a missing path.
fn check_no_symlink_escape(
    phys_target: &Path,
    normalized_target: &str,
    normalized: &str,
) -> std::result::Result<(), FsErrorKind> {
    let rel = normalized
        .strip_prefix(normalized_target)
        .unwrap_or(normalized)
        .trim_start_matches('/');
    let mut cursor = phys_target.to_path_buf();
    for component in Path::new(rel).components() {
        if let Component::Normal(name) = component {
            cursor.push(name);
            if let Ok(meta) = std::fs::symlink_metadata(&cursor) {
                if meta.file_type().is_symlink() {
                    return Err(FsErrorKind::AccessDenied {
                        path: normalized.to_string(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Normalize a POSIX-style path: strip `.` segments, resolve `..` against
/// preceding segments, and collapse multiple slashes. Leading `/` is preserved
/// (required by the prefix check); `..` segments that would escape the root
/// are clamped at the root.
fn normalize_path(input: &str) -> String {
    let absolute = input.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for seg in input.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            s => stack.push(s),
        }
    }
    if absolute {
        format!("/{}", stack.join("/"))
    } else {
        stack.join("/")
    }
}

/// Error categories we report back to JavaScript. Each maps to a Node-style
/// `err.code` string.
#[derive(Debug)]
#[allow(dead_code)] // IsDirectory/NotDirectory reserved for future stat-driven paths
enum FsErrorKind {
    AccessDenied { path: String },
    NotFound { path: String },
    InvalidPath { path: String },
    IsDirectory { path: String },
    NotDirectory { path: String },
    Io { message: String, path: String },
}

impl FsErrorKind {
    fn code(&self) -> &'static str {
        match self {
            FsErrorKind::AccessDenied { .. } => "EACCES",
            FsErrorKind::NotFound { .. } => "ENOENT",
            FsErrorKind::InvalidPath { .. } => "EINVAL",
            FsErrorKind::IsDirectory { .. } => "EISDIR",
            FsErrorKind::NotDirectory { .. } => "ENOTDIR",
            FsErrorKind::Io { .. } => "EIO",
        }
    }

    fn path(&self) -> &str {
        match self {
            FsErrorKind::AccessDenied { path }
            | FsErrorKind::NotFound { path }
            | FsErrorKind::InvalidPath { path }
            | FsErrorKind::IsDirectory { path }
            | FsErrorKind::NotDirectory { path }
            | FsErrorKind::Io { path, .. } => path,
        }
    }

    fn message(&self, syscall: &str) -> String {
        let path = self.path();
        match self {
            FsErrorKind::AccessDenied { .. } => {
                format!("EACCES: permission denied, {syscall} '{path}'")
            }
            FsErrorKind::NotFound { .. } => {
                format!("ENOENT: no such file or directory, {syscall} '{path}'")
            }
            FsErrorKind::InvalidPath { .. } => {
                format!("EINVAL: invalid argument, {syscall} '{path}'")
            }
            FsErrorKind::IsDirectory { .. } => {
                format!("EISDIR: illegal operation on a directory, {syscall} '{path}'")
            }
            FsErrorKind::NotDirectory { .. } => {
                format!("ENOTDIR: not a directory, {syscall} '{path}'")
            }
            FsErrorKind::Io { message, .. } => {
                format!("EIO: {message}, {syscall} '{path}'")
            }
        }
    }
}

fn throw_fs(ctx: &Ctx<'_>, kind: FsErrorKind, syscall: &str) -> Error {
    let message = kind.message(syscall);
    let exc = match Exception::from_message(ctx.clone(), &message) {
        Ok(e) => e,
        Err(e) => return e,
    };
    let obj = exc.as_object();
    let _ = obj.set("code", kind.code());
    let _ = obj.set("syscall", syscall);
    let _ = obj.set("path", kind.path());
    exc.throw()
}

fn map_vfs_err(err: VfsError, path: &str) -> FsErrorKind {
    match err.kind() {
        VfsErrorKind::FileNotFound => FsErrorKind::NotFound {
            path: path.to_string(),
        },
        VfsErrorKind::InvalidPath => FsErrorKind::InvalidPath {
            path: path.to_string(),
        },
        _ => FsErrorKind::Io {
            message: err.to_string(),
            path: path.to_string(),
        },
    }
}

fn config(ctx: &Ctx<'_>) -> Result<CuratedFsConfig> {
    ctx.userdata::<CuratedFsConfig>()
        .map(|r| r.clone())
        .ok_or_else(|| {
            Exception::throw_message(ctx, "CuratedFsConfig not installed in runtime userdata")
        })
}

// ---------- encoding helpers ----------

fn encoding_from_options<'js>(options: Opt<Value<'js>>) -> Option<String> {
    let value = options.0?;
    if let Some(s) = value.as_string() {
        return s.to_string().ok();
    }
    if let Some(obj) = value.as_object() {
        if let Ok(enc) = obj.get::<_, Option<String>>("encoding") {
            return enc;
        }
    }
    None
}

fn recursive_from_options<'js>(options: Opt<Value<'js>>) -> bool {
    match options.0 {
        Some(value) => value
            .as_object()
            .and_then(|obj| obj.get::<_, Option<bool>>("recursive").ok().flatten())
            .unwrap_or(false),
        None => false,
    }
}

// ---------- sync ops ----------

/// Read raw bytes for `normalized` from `cfg`'s VFS, falling back to
/// `cfg.fetcher` on FileNotFound. Bytes fetched from the fallback are
/// written back into the VFS so subsequent reads (including from other
/// workers sharing `cfg.root`) are pure memory hits.
fn read_bytes_via_vfs_or_fetcher(
    ctx: &Ctx<'_>,
    cfg: &CuratedFsConfig,
    normalized: &str,
    vfs_path: &VfsPath,
) -> Result<Vec<u8>> {
    match vfs_path.open_file() {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| {
                throw_fs(
                    ctx,
                    FsErrorKind::Io {
                        message: e.to_string(),
                        path: normalized.to_string(),
                    },
                    "read",
                )
            })?;
            Ok(bytes)
        }
        Err(err) => {
            if !matches!(err.kind(), VfsErrorKind::FileNotFound) {
                return Err(throw_fs(ctx, map_vfs_err(err, normalized), "open"));
            }
            let Some(fetcher) = &cfg.fetcher else {
                return Err(throw_fs(
                    ctx,
                    FsErrorKind::NotFound {
                        path: normalized.to_string(),
                    },
                    "open",
                ));
            };
            match fetcher.fetch(normalized) {
                Ok(Some(bytes)) => {
                    // Populate the VFS so the next read skips the fetcher
                    // entirely (and so other workers sharing this VFS see
                    // the file immediately).
                    if let Some(parent) = parent_of(normalized) {
                        if let Ok(parent_vfs) = cfg.root.join(parent.trim_start_matches('/')) {
                            let _ = parent_vfs.create_dir_all();
                        }
                    }
                    // Another worker raced us — if the file now exists,
                    // proceed with our freshly fetched bytes and let
                    // their write stand. Any other write failure is
                    // non-fatal for the current read.
                    if let Ok(mut f) = vfs_path.create_file() {
                        let _ = f.write_all(&bytes);
                    }
                    Ok(bytes)
                }
                Ok(None) => Err(throw_fs(
                    ctx,
                    FsErrorKind::NotFound {
                        path: normalized.to_string(),
                    },
                    "open",
                )),
                Err(message) => Err(throw_fs(
                    ctx,
                    FsErrorKind::Io {
                        message,
                        path: normalized.to_string(),
                    },
                    "open",
                )),
            }
        }
    }
}

fn read_file_sync_impl<'js>(
    ctx: &Ctx<'js>,
    path: &str,
    encoding: Option<String>,
) -> Result<Value<'js>> {
    let cfg = config(ctx)?;
    let (normalized, vfs_path) = cfg.resolve(path).map_err(|e| throw_fs(ctx, e, "open"))?;
    let bytes = read_bytes_via_vfs_or_fetcher(ctx, &cfg, &normalized, &vfs_path)?;
    if encoding.is_some() {
        // Node decodes as UTF-8 when `encoding` is "utf-8"/"utf8". For any
        // other encoding we still hand back a string — the curated fs MVP
        // doesn't implement per-encoding decoders. Codemods operating on
        // text source don't observe a difference.
        let s = String::from_utf8_lossy(&bytes).into_owned();
        s.into_js(ctx)
    } else {
        // Match Node semantics: without `encoding`, return a Uint8Array so
        // callers that `Buffer.from(...)` or otherwise treat the result as
        // binary data behave the same as they do under Node.
        TypedArray::<u8>::new(ctx.clone(), bytes).map(|ta| ta.into_value())
    }
}

fn read_file_sync<'js>(
    ctx: Ctx<'js>,
    path: String,
    options: Opt<Value<'js>>,
) -> Result<Value<'js>> {
    let encoding = encoding_from_options(options);
    read_file_sync_impl(&ctx, &path, encoding)
}

fn write_file_sync_impl(ctx: &Ctx<'_>, path: &str, data: String) -> Result<()> {
    let cfg = config(ctx)?;
    let (normalized, vfs_path) = cfg.resolve(path).map_err(|e| throw_fs(ctx, e, "open"))?;
    if let Some(parent) = parent_of(&normalized) {
        let parent_vfs = cfg.root.join(parent.trim_start_matches('/')).map_err(|_| {
            throw_fs(
                ctx,
                FsErrorKind::InvalidPath {
                    path: parent.clone(),
                },
                "mkdir",
            )
        })?;
        parent_vfs
            .create_dir_all()
            .map_err(|e| throw_fs(ctx, map_vfs_err(e, &parent), "mkdir"))?;
    }
    let mut file = vfs_path
        .create_file()
        .map_err(|e| throw_fs(ctx, map_vfs_err(e, &normalized), "open"))?;
    file.write_all(data.as_bytes()).map_err(|e| {
        throw_fs(
            ctx,
            FsErrorKind::Io {
                message: e.to_string(),
                path: normalized.clone(),
            },
            "write",
        )
    })?;
    Ok(())
}

fn write_file_sync(
    ctx: Ctx<'_>,
    path: String,
    data: String,
    _options: Opt<Value<'_>>,
) -> Result<()> {
    write_file_sync_impl(&ctx, &path, data)
}

fn exists_sync(ctx: Ctx<'_>, path: String) -> Result<bool> {
    let cfg = config(&ctx)?;
    let (_, vfs_path) = match cfg.resolve(&path) {
        Ok(p) => p,
        // Paths outside target_dir are reported as non-existent rather than
        // throwing — matches Node's `existsSync` which never throws.
        Err(_) => return Ok(false),
    };
    Ok(vfs_path.exists().unwrap_or(false))
}

fn readdir_sync(ctx: Ctx<'_>, path: String, _options: Opt<Value<'_>>) -> Result<Vec<String>> {
    let cfg = config(&ctx)?;
    let (normalized, vfs_path) = cfg
        .resolve(&path)
        .map_err(|e| throw_fs(&ctx, e, "scandir"))?;
    let iter = vfs_path
        .read_dir()
        .map_err(|e| throw_fs(&ctx, map_vfs_err(e, &normalized), "scandir"))?;
    let prefix = if normalized.ends_with('/') {
        normalized.clone()
    } else {
        format!("{normalized}/")
    };
    let mut entries: Vec<String> = iter
        .map(|p| {
            let s = p.as_str().to_string();
            s.strip_prefix(&prefix).unwrap_or(&s).to_string()
        })
        .collect();
    entries.sort();
    Ok(entries)
}

fn mkdir_sync(ctx: Ctx<'_>, path: String, options: Opt<Value<'_>>) -> Result<()> {
    let cfg = config(&ctx)?;
    let (normalized, vfs_path) = cfg.resolve(&path).map_err(|e| throw_fs(&ctx, e, "mkdir"))?;
    let recursive = recursive_from_options(options);
    let result = if recursive {
        vfs_path.create_dir_all()
    } else {
        vfs_path.create_dir()
    };
    result.map_err(|e| throw_fs(&ctx, map_vfs_err(e, &normalized), "mkdir"))?;
    Ok(())
}

fn stat_sync<'js>(ctx: Ctx<'js>, path: String) -> Result<Object<'js>> {
    let cfg = config(&ctx)?;
    let (normalized, vfs_path) = cfg.resolve(&path).map_err(|e| throw_fs(&ctx, e, "stat"))?;
    let meta = vfs_path
        .metadata()
        .map_err(|e| throw_fs(&ctx, map_vfs_err(e, &normalized), "stat"))?;
    stats_object(&ctx, meta.file_type, meta.len)
}

fn unlink_sync(ctx: Ctx<'_>, path: String) -> Result<()> {
    let cfg = config(&ctx)?;
    let (normalized, vfs_path) = cfg
        .resolve(&path)
        .map_err(|e| throw_fs(&ctx, e, "unlink"))?;
    vfs_path
        .remove_file()
        .map_err(|e| throw_fs(&ctx, map_vfs_err(e, &normalized), "unlink"))?;
    Ok(())
}

fn stats_object<'js>(ctx: &Ctx<'js>, file_type: VfsFileType, size: u64) -> Result<Object<'js>> {
    let obj = Object::new(ctx.clone())?;
    obj.set("size", size as f64)?;
    let is_file = matches!(file_type, VfsFileType::File);
    let is_dir = matches!(file_type, VfsFileType::Directory);
    let is_file_fn = rquickjs::Function::new(ctx.clone(), move || is_file)?;
    let is_dir_fn = rquickjs::Function::new(ctx.clone(), move || is_dir)?;
    obj.set("isFile", is_file_fn)?;
    obj.set("isDirectory", is_dir_fn)?;
    Ok(obj)
}

fn parent_of(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    let idx = trimmed.rfind('/')?;
    if idx == 0 {
        Some("/".to_string())
    } else {
        Some(trimmed[..idx].to_string())
    }
}

// ---------- promise (async) wrappers ----------
//
// The VFS API is synchronous, so the async variants simply wrap the sync
// logic in an async fn. rquickjs turns the returned future into a JS Promise.

async fn read_file<'js>(
    ctx: Ctx<'js>,
    path: String,
    options: Opt<Value<'js>>,
) -> Result<Value<'js>> {
    let encoding = encoding_from_options(options);
    read_file_sync_impl(&ctx, &path, encoding)
}

async fn write_file(
    ctx: Ctx<'_>,
    path: String,
    data: String,
    _options: Opt<Value<'_>>,
) -> Result<()> {
    write_file_sync_impl(&ctx, &path, data)
}

async fn readdir_async(ctx: Ctx<'_>, path: String, options: Opt<Value<'_>>) -> Result<Vec<String>> {
    readdir_sync(ctx, path, options)
}

async fn mkdir_async(ctx: Ctx<'_>, path: String, options: Opt<Value<'_>>) -> Result<()> {
    mkdir_sync(ctx, path, options)
}

async fn stat_async<'js>(ctx: Ctx<'js>, path: String) -> Result<Object<'js>> {
    stat_sync(ctx, path)
}

async fn unlink_async(ctx: Ctx<'_>, path: String) -> Result<()> {
    unlink_sync(ctx, path)
}

// ---------- module registration ----------

/// `fs` module — exports the sync API and a `promises` sub-object.
pub struct CuratedFsModule;

impl ModuleDef for CuratedFsModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("readFileSync")?;
        declare.declare("writeFileSync")?;
        declare.declare("existsSync")?;
        declare.declare("readdirSync")?;
        declare.declare("mkdirSync")?;
        declare.declare("statSync")?;
        declare.declare("unlinkSync")?;
        declare.declare("promises")?;
        declare.declare("default")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        // Verify the config is present up front so import-time failures are
        // clear rather than showing up on first method call.
        let _ = config(ctx)?;

        let default = Object::new(ctx.clone())?;
        let promises = build_promises_object(ctx)?;

        default.set("readFileSync", Func::from(read_file_sync))?;
        default.set("writeFileSync", Func::from(write_file_sync))?;
        default.set("existsSync", Func::from(exists_sync))?;
        default.set("readdirSync", Func::from(readdir_sync))?;
        default.set("mkdirSync", Func::from(mkdir_sync))?;
        default.set("statSync", Func::from(stat_sync))?;
        default.set("unlinkSync", Func::from(unlink_sync))?;
        default.set("promises", promises.clone())?;

        exports.export("readFileSync", Func::from(read_file_sync))?;
        exports.export("writeFileSync", Func::from(write_file_sync))?;
        exports.export("existsSync", Func::from(exists_sync))?;
        exports.export("readdirSync", Func::from(readdir_sync))?;
        exports.export("mkdirSync", Func::from(mkdir_sync))?;
        exports.export("statSync", Func::from(stat_sync))?;
        exports.export("unlinkSync", Func::from(unlink_sync))?;
        exports.export("promises", promises)?;
        exports.export("default", default)?;
        Ok(())
    }
}

/// `fs/promises` module — exports the async/promise-returning variants.
pub struct CuratedFsPromisesModule;

impl ModuleDef for CuratedFsPromisesModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("readFile")?;
        declare.declare("writeFile")?;
        declare.declare("readdir")?;
        declare.declare("mkdir")?;
        declare.declare("stat")?;
        declare.declare("unlink")?;
        declare.declare("default")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let _ = config(ctx)?;
        let default = build_promises_object(ctx)?;

        exports.export("readFile", Func::from(Async(read_file)))?;
        exports.export("writeFile", Func::from(Async(write_file)))?;
        exports.export("readdir", Func::from(Async(readdir_async)))?;
        exports.export("mkdir", Func::from(Async(mkdir_async)))?;
        exports.export("stat", Func::from(Async(stat_async)))?;
        exports.export("unlink", Func::from(Async(unlink_async)))?;
        exports.export("default", default)?;
        Ok(())
    }
}

fn build_promises_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>> {
    let promises = Object::new(ctx.clone())?;
    promises.set("readFile", Func::from(Async(read_file)))?;
    promises.set("writeFile", Func::from(Async(write_file)))?;
    promises.set("readdir", Func::from(Async(readdir_async)))?;
    promises.set("mkdir", Func::from(Async(mkdir_async)))?;
    promises.set("stat", Func::from(Async(stat_async)))?;
    promises.set("unlink", Func::from(Async(unlink_async)))?;
    Ok(promises)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vfs::MemoryFS;

    fn make_config() -> CuratedFsConfig {
        CuratedFsConfig::new("/app", MemoryFS::new().into())
    }

    #[test]
    fn resolve_allows_path_under_target() {
        let cfg = make_config();
        let (normalized, _) = cfg.resolve("/app/src/foo.ts").unwrap();
        assert_eq!(normalized, "/app/src/foo.ts");
    }

    #[test]
    fn resolve_joins_relative_under_target() {
        let cfg = make_config();
        let (normalized, _) = cfg.resolve("src/foo.ts").unwrap();
        assert_eq!(normalized, "/app/src/foo.ts");
    }

    #[test]
    fn resolve_rejects_outside_target() {
        let cfg = make_config();
        match cfg.resolve("/etc/passwd") {
            Err(FsErrorKind::AccessDenied { path }) => assert_eq!(path, "/etc/passwd"),
            other => panic!("expected AccessDenied, got {other:?}"),
        }
    }

    #[test]
    fn resolve_rejects_dotdot_escape() {
        let cfg = make_config();
        match cfg.resolve("/app/../etc/passwd") {
            Err(FsErrorKind::AccessDenied { path }) => assert_eq!(path, "/etc/passwd"),
            other => panic!("expected AccessDenied, got {other:?}"),
        }
    }

    #[test]
    fn normalize_strips_dots_and_resolves_dotdot() {
        assert_eq!(normalize_path("/app/src/./foo.ts"), "/app/src/foo.ts");
        assert_eq!(normalize_path("/app/src/../src/foo.ts"), "/app/src/foo.ts");
        assert_eq!(normalize_path("/app/../etc/passwd"), "/etc/passwd");
        assert_eq!(normalize_path("/app//src///foo.ts"), "/app/src/foo.ts");
    }

    /// When `physical_target_dir` is set, traversing a symlink — even one
    /// whose lexical form stays under `target_dir` — must be rejected. This
    /// is the gap pure lexical normalization leaves open on real disk.
    #[cfg(unix)]
    #[test]
    fn resolve_with_physical_target_dir_rejects_symlink_traversal() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret"), "leaked").unwrap();
        symlink(&outside, repo.join("link")).unwrap();

        let repo_str = repo.to_string_lossy().into_owned();
        let cfg = CuratedFsConfig::new(repo_str.clone(), MemoryFS::new().into())
            .with_physical_target_dir(repo.clone());

        // Lexically under target_dir, but traverses a symlink — must be
        // rejected so the codemod can't read `outside/secret`.
        let traversal = format!("{repo_str}/link/secret");
        match cfg.resolve(&traversal) {
            Err(FsErrorKind::AccessDenied { path }) => assert_eq!(path, traversal),
            other => panic!("expected AccessDenied, got {other:?}"),
        }

        // A non-existent path under target_dir is fine — no symlink can
        // live at a path that doesn't exist, and writes need to resolve
        // parents even when the file itself is absent.
        let fresh = format!("{repo_str}/fresh.ts");
        assert!(cfg.resolve(&fresh).is_ok());

        // Regular files inside target_dir still resolve.
        std::fs::write(repo.join("ok.ts"), "ok").unwrap();
        let ok = format!("{repo_str}/ok.ts");
        assert!(cfg.resolve(&ok).is_ok());
    }

    /// Without `physical_target_dir` (in-memory VFS backends),
    /// the symlink check is skipped — MemoryFS has no symlinks, and poking
    /// the host filesystem would be both wrong and slow.
    #[cfg(unix)]
    #[test]
    fn resolve_without_physical_target_dir_skips_symlink_check() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        symlink(temp.path().join("outside"), repo.join("link")).unwrap();

        // target_dir matches the host repo path but `physical_target_dir`
        // is intentionally unset — this is the in-memory engine's profile.
        let repo_str = repo.to_string_lossy().into_owned();
        let cfg = CuratedFsConfig::new(repo_str.clone(), MemoryFS::new().into());

        // Without the symlink check, lexical resolution alone succeeds; the
        // VFS (here MemoryFS) is what ultimately serves the read and it has
        // no concept of the host symlink.
        let traversal = format!("{repo_str}/link/secret");
        assert!(cfg.resolve(&traversal).is_ok());
    }
}

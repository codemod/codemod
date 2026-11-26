//! Target-scoped file access for transforms and callbacks.

use std::collections::BTreeSet;
use std::io;
use std::path::{Component, Path, PathBuf};

use globset::GlobBuilder;
use rquickjs::loader::Resolver;
use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::prelude::{Async, Func};
use rquickjs::{Ctx, Error, Exception, JsLifetime, Object, Result};

use super::PendingFiles;

pub const FS_REPLACEMENT: &str =
    "fs is not available in 2.0 workflows; use `import { files } from \"codemod/files\"` inside a callback";

/// Marks the replayed workflow body, where target reads are not allowed.
pub struct WorkflowBody;

unsafe impl<'js> JsLifetime<'js> for WorkflowBody {
    type Changed<'to> = WorkflowBody;
}

pub fn is_fs(name: &str) -> bool {
    matches!(
        name.strip_prefix("node:").unwrap_or(name),
        "fs" | "fs/promises"
    )
}

/// Run before the builtin and package resolvers.
pub struct WorkflowFsResolver(pub bool);

impl Resolver for WorkflowFsResolver {
    fn resolve(&mut self, _ctx: &Ctx<'_>, base: &str, name: &str) -> Result<String> {
        if is_fs(name) {
            if self.0 {
                return Ok(if name.ends_with("/promises") {
                    "codemod-internal:fs-promises"
                } else {
                    "codemod-internal:fs-compat"
                }
                .into());
            }
            return Err(Error::new_loading_message(name, FS_REPLACEMENT));
        }
        Err(Error::new_resolving(base, name))
    }
}

#[derive(Clone)]
pub struct TargetFiles {
    target: PathBuf,
    root: PathBuf,
    pending: PendingFiles,
}

// No references to JS values are stored here.
unsafe impl<'js> JsLifetime<'js> for TargetFiles {
    type Changed<'to> = TargetFiles;
}

impl TargetFiles {
    pub fn new(target: PathBuf, pending: PendingFiles) -> Self {
        let root = dunce::canonicalize(&target).unwrap_or_else(|_| target.clone());
        Self {
            target,
            root,
            pending,
        }
    }

    pub fn created(&self) -> Vec<(String, String)> {
        self.pending
            .created()
            .into_iter()
            .filter_map(|(path, text)| Some((slash(path.strip_prefix(&self.root).ok()?), text)))
            .collect()
    }

    /// Resolve relative and target-local absolute paths, including new paths.
    /// A symlink isn't part of the workflow's file view.
    fn resolve(&self, input: &str) -> std::result::Result<PathBuf, String> {
        let outside =
            || format!("files: '{input}' must stay inside the target (symlinks are not followed)");
        let input = input.replace('\\', "/");
        let path = Path::new(&input);
        let relative = if path.is_absolute() {
            path.strip_prefix(&self.root)
                .or_else(|_| path.strip_prefix(&self.target))
                .map_err(|_| outside())?
        } else {
            path
        };
        let mut full = self.root.clone();
        for part in relative.components() {
            match part {
                Component::CurDir => {}
                Component::ParentDir if full != self.root => {
                    full.pop();
                }
                Component::Normal(part) => {
                    full.push(part);
                    match std::fs::symlink_metadata(&full) {
                        Ok(metadata) if metadata.is_symlink() => return Err(outside()),
                        Ok(_) => {}
                        Err(e) if missing(&e) => {}
                        Err(e) => return Err(format!("files: '{input}': {e}")),
                    }
                }
                _ => return Err(outside()),
            }
        }
        Ok(full)
    }

    pub fn read(&self, path: &str) -> std::result::Result<String, String> {
        let full = self.resolve(path)?;
        match self.kind(&full, false)? {
            "file" => self
                .pending
                .read(&full)
                .map_err(|e| format!("files.read('{path}'): {e}")),
            "directory" => Err(format!("files.read('{path}'): it is a directory")),
            _ => Err(format!("files.read('{path}'): no such file")),
        }
    }

    pub fn exists(&self, path: &str) -> std::result::Result<bool, String> {
        Ok(self.kind(&self.resolve(path)?, true)? != "missing")
    }

    pub fn create(&self, path: &str, text: String) -> std::result::Result<(), String> {
        let full = self.resolve(path)?;
        if self.kind(&full, true)? != "missing" {
            return Err(format!("files.create('{path}'): already exists"));
        }
        for parent in full.ancestors().skip(1).take_while(|p| *p != self.root) {
            if self.kind(parent, false)? == "file" {
                return Err(format!(
                    "files.create('{path}'): '{}' is a file",
                    parent.display()
                ));
            }
        }
        self.pending
            .create(full, text)
            .map_err(|e| format!("files.create('{path}'): {e}"))
    }

    pub fn list(&self, pattern: &str) -> std::result::Result<Vec<String>, String> {
        let pattern = pattern.replace('\\', "/");
        if pattern.starts_with('/')
            || pattern.as_bytes().get(1) == Some(&b':')
            || pattern.split('/').any(|part| part == "..")
        {
            return Err(format!(
                "files.list('{pattern}'): the glob must stay inside the target"
            ));
        }
        let glob = GlobBuilder::new(pattern.trim_start_matches("./"))
            .literal_separator(true)
            .build()
            .map_err(|e| format!("files.list('{pattern}'): {e}"))?
            .compile_matcher();
        let mut paths = BTreeSet::new();
        let mut walk = ignore::WalkBuilder::new(&self.root);
        walk.standard_filters(false)
            .filter_entry(|entry| entry.file_name() != ".git");
        for entry in walk.build() {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().is_some_and(|kind| kind.is_file()) {
                let full = entry.path();
                if let Ok(relative) = full.strip_prefix(&self.root) {
                    if glob.is_match(relative) && self.pending.presence(full) != Some(false) {
                        paths.insert(slash(relative));
                    }
                }
            }
        }
        for full in self.pending.new_paths() {
            if let Ok(relative) = full.strip_prefix(&self.root) {
                if glob.is_match(relative) {
                    paths.insert(slash(relative));
                }
            }
        }
        Ok(paths.into_iter().collect())
    }

    fn kind(
        &self,
        full: &Path,
        check_pending_dirs: bool,
    ) -> std::result::Result<&'static str, String> {
        match self.pending.presence(full) {
            Some(true) => return Ok("file"),
            Some(false) => return Ok("missing"),
            None => {}
        }
        match std::fs::metadata(full) {
            Ok(meta) if meta.is_file() => return Ok("file"),
            Ok(meta) if meta.is_dir() => return Ok("directory"),
            Ok(_) => return Ok("other"),
            Err(e) if missing(&e) => {}
            Err(e) => return Err(format!("files: '{}': {e}", full.display())),
        }
        if check_pending_dirs
            && self
                .pending
                .new_paths()
                .iter()
                .any(|path| path.starts_with(full))
        {
            return Ok("directory");
        }
        Ok("missing")
    }

    fn dir_entries(&self, path: &str) -> std::result::Result<Vec<(String, &'static str)>, String> {
        let full = self.resolve(path)?;
        if self.kind(&full, true)? != "directory" {
            return Err(format!("readdir('{path}'): not a directory"));
        }
        let mut names = BTreeSet::new();
        match std::fs::read_dir(&full) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|e| e.to_string())?;
                    if !entry.file_type().map_err(|e| e.to_string())?.is_symlink() {
                        names.insert(entry.file_name().to_string_lossy().into_owned());
                    }
                }
            }
            Err(e) if missing(&e) => {}
            Err(e) => return Err(e.to_string()),
        }
        for path in self.pending.new_paths() {
            if let Some(Component::Normal(name)) = path
                .strip_prefix(&full)
                .ok()
                .and_then(|p| p.components().next())
            {
                names.insert(name.to_string_lossy().into_owned());
            }
        }
        let mut entries = Vec::new();
        for name in names {
            let kind = self.kind(&full.join(&name), true)?;
            if kind != "missing" {
                entries.push((name, kind));
            }
        }
        Ok(entries)
    }
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn missing(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn with_files<'js, T>(
    ctx: &Ctx<'js>,
    f: impl FnOnce(&TargetFiles) -> std::result::Result<T, String>,
) -> Result<T> {
    let files = ctx.userdata::<TargetFiles>().ok_or_else(|| {
        Exception::throw_message(
            ctx,
            "files is only available in a workflow callback or a JSSG transform",
        )
    })?;
    f(&files).map_err(|e| Exception::throw_message(ctx, &e))
}

fn read(ctx: Ctx<'_>, path: String) -> Result<String> {
    with_files(&ctx, |files| files.read(&path))
}

fn exists(ctx: Ctx<'_>, path: String) -> Result<bool> {
    with_files(&ctx, |files| files.exists(&path))
}

fn list(ctx: Ctx<'_>, glob: String) -> Result<Vec<String>> {
    with_files(&ctx, |files| files.list(&glob))
}

fn create(ctx: Ctx<'_>, path: String, text: String) -> Result<()> {
    with_files(&ctx, |files| files.create(&path, text))
}

fn kind(ctx: Ctx<'_>, path: String) -> Result<&'static str> {
    with_files(&ctx, |files| files.kind(&files.resolve(&path)?, true))
}

fn directory(ctx: Ctx<'_>, path: String) -> Result<String> {
    with_files(&ctx, |files| {
        files
            .dir_entries(&path)
            .map(|entries| serde_json::json!(entries).to_string())
    })
}

fn mkdir(ctx: Ctx<'_>, path: String) -> Result<()> {
    // Legacy mkdir only validates the target boundary. If a later write is
    // recorded, its parent directories are created when the step commits.
    with_files(&ctx, |files| files.resolve(&path).map(|_| ()))
}

async fn defer() {
    tokio::task::yield_now().await;
}

pub struct FilesModule;

impl ModuleDef for FilesModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("files")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let files = Object::new(ctx.clone())?;
        files.set("read", Func::from(read))?;
        files.set("exists", Func::from(exists))?;
        files.set("list", Func::from(list))?;
        files.set("create", Func::from(create))?;
        let freeze: rquickjs::Function = ctx.eval("Object.freeze")?;
        let files: Object = freeze.call((files,))?;
        exports.export("files", files)?;
        Ok(())
    }
}

pub(super) struct CompatFsHost;

impl ModuleDef for CompatFsHost {
    fn declare(declare: &Declarations) -> Result<()> {
        declare
            .declare("kind")?
            .declare("directory")?
            .declare("mkdir")?
            .declare("defer")?;
        Ok(())
    }

    fn evaluate<'js>(_: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        exports
            .export("kind", Func::from(kind))?
            .export("directory", Func::from(directory))?
            .export("mkdir", Func::from(mkdir))?
            .export("defer", Func::from(Async(defer)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_are_visible_in_their_transaction_and_rolled_back_on_failure() {
        let target = tempfile::tempdir().unwrap();
        let pending = PendingFiles::default();
        let files = TargetFiles::new(target.path().to_path_buf(), pending.clone());
        let transaction = pending.begin();
        files.create("dir/new.txt", "text".to_string()).unwrap();
        assert!(files.exists("dir").unwrap());
        assert_eq!(files.read("dir/new.txt").unwrap(), "text");
        assert_eq!(files.list("**/*.txt").unwrap(), ["dir/new.txt"]);
        assert!(files.create("dir", String::new()).is_err());
        assert!(files.create("dir/new.txt", String::new()).is_err());
        transaction.finish(false);
        assert!(!files.exists("dir").unwrap());
        assert!(files.created().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn files_does_not_follow_symlinks() {
        let target = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(outside.path(), target.path().join("link")).unwrap();
        let files = TargetFiles::new(target.path().to_path_buf(), PendingFiles::default());
        assert!(files
            .read("link/secret.txt")
            .unwrap_err()
            .contains("inside the target"));
        assert!(files
            .exists("link/secret.txt")
            .unwrap_err()
            .contains("inside the target"));
        assert!(files
            .create("link/new.txt", String::new())
            .unwrap_err()
            .contains("inside the target"));
        assert!(files.list("**/*.txt").unwrap().is_empty());
        assert!(files
            .list("../*.txt")
            .unwrap_err()
            .contains("inside the target"));
    }
}

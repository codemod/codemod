//! Native file selection for the TypeScript orchestration runtimes.
//!
//! [`select`] is `walk()` of `packages/orchestration/src/execution/files.ts`
//! in Rust, and [`read_sources`] is `readSource` of
//! `src/execution/jssg.ts` over a whole selection. A `Host` that offers them
//! (`Host.selectFiles`, `Host.fs.readSources`; the QuickJS host does) lets
//! `files.ts` skip interpreting the walk. The contract is exact equality with
//! the TypeScript walker, file list and errors, which the quickjs e2e tests
//! check by running the same workflows with `CODEMOD_NATIVE_SELECT=0` (the
//! TypeScript walk inside QuickJS) on a tree built to be awkward.
//!
//! This is deliberately not the engine's walker (`codemod_walk_builder`, the
//! `ignore` crate). The TypeScript walker has semantics of its own that the
//! crate's matcher does not reproduce: matching is the `ignore` npm package
//! (case-insensitive JS regular expressions, a path is ignored when any
//! parent directory is, see [`node_ignore`]), two override sets (definition
//! globs relative to the repository, invocation globs relative to the
//! target), and `.ignore` / `.gitignore` / `.git/info/exclude` read in every
//! ancestor of the target. `fixtures/walker/cases.json` pins where both
//! agree with the engine.
//!
//! Paths are strings, exactly as the TypeScript walker sees them through the
//! host: directory entry names are decoded lossily, and every path (to list a
//! directory, read an ignore file, or read a source) is built from those
//! strings, so a name that is not UTF-8 behaves as it does in TypeScript.
//! The caller passes the inputs `files.ts` derives in TypeScript: the walk
//! root and override roots (absolute, normalized, each override root equal to
//! or an ancestor of the walk root), the override patterns (braces expanded,
//! includes negated), and the global excludes file already discovered.

pub mod node_ignore;

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering as Atomic};
use std::sync::{Arc, Mutex};

use node_ignore::{Ignore, Match, PathError, PatternError, RuleCache};

/// One include/exclude set (`Override` in `files.ts`).
#[derive(Debug, Clone)]
pub struct OverrideSet {
    /// Absolute directory the patterns are relative to.
    pub root: String,
    /// `ignore()` lines: excludes as written, includes as `!glob`.
    pub patterns: Vec<String>,
    /// At least one include: an unmatched file is ignored.
    pub whitelists: bool,
}

#[derive(Debug, Clone)]
pub struct Request {
    /// Absolute, normalized directory to walk.
    pub root: String,
    pub overrides: Vec<OverrideSet>,
    /// Global git excludes file, already discovered; `None` disables it.
    pub global_excludes: Option<String>,
}

#[derive(Debug)]
pub enum Error {
    /// A host filesystem call failed: `op` is the Node-style syscall name.
    Io {
        op: &'static str,
        path: String,
        error: io::Error,
    },
    /// The `ignore` package refused a path (`checkPath`).
    Path(PathError),
    /// An override pattern does not compile.
    Pattern(PatternError),
    /// A path the walker produced is not beneath an override root.
    NotBeneath { base: String, path: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io { op, path, error } => write!(f, "{error}, {op} '{path}'"),
            Error::Path(error) => error.fmt(f),
            Error::Pattern(error) => error.fmt(f),
            Error::NotBeneath { base, path } => write!(f, "'{path}' is not beneath '{base}'"),
        }
    }
}

impl std::error::Error for Error {}

type Matcher = Option<Ignore>;

struct Level {
    dir: String,
    /// `.ignore`, `.gitignore`, `.git/info/exclude`, in precedence order.
    kinds: [Matcher; 3],
}

/// The `levels` array of `files.ts` as a shared chain, nearest first.
struct Levels {
    level: Level,
    parent: Option<Arc<Levels>>,
}

impl Levels {
    fn iter(&self) -> impl Iterator<Item = &Level> {
        std::iter::successors(Some(self), |node| node.parent.as_deref()).map(|node| &node.level)
    }
}

/// One selection. Subdirectories are walked on scoped threads while spare
/// ones remain (`spare`); every directory's result is combined in listing
/// order, so the file set and the error reported (the first one the
/// sequential TypeScript walk would hit) are the same as walking serially.
struct Walk<'a> {
    root: &'a str,
    overrides: Vec<(&'a str, Ignore, bool)>,
    global: Matcher,
    cache: Mutex<RuleCache>,
    spare: AtomicUsize,
}

/// POSIX `dirname` of an absolute, normalized path.
fn dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(index) => &path[..index],
    }
}

/// POSIX `join(dir, name)` for a single entry name.
fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// `candidate(base, absolute)` of `files.ts` for paths beneath `base`.
fn candidate<'p>(base: &str, absolute: &'p str) -> Result<&'p str, Error> {
    if let Some(rest) = absolute.strip_prefix(base) {
        if rest.is_empty() {
            return Ok(rest);
        }
        if base.ends_with('/') {
            return Ok(rest);
        }
        if let Some(rest) = rest.strip_prefix('/') {
            return Ok(rest);
        }
    }
    Err(Error::NotBeneath {
        base: base.to_string(),
        path: absolute.to_string(),
    })
}

/// `test(matcher, candidate, isDir)` of `files.ts`.
fn test(matcher: &Ignore, candidate: &str, is_dir: bool) -> Result<Match, Error> {
    if is_dir {
        matcher.test(&format!("{candidate}/"))
    } else {
        matcher.test(candidate)
    }
    .map_err(Error::Path)
}

/// Host `readText` (lossy UTF-8).
fn read_lossy(path: &str) -> io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    })
}

impl Walk<'_> {
    /// `readIgnoreFile`: unreadable or not compilable means no matcher.
    fn read_ignore_file(&self, path: &str) -> Matcher {
        let text = read_lossy(path).ok()?;
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        Ignore::from_text(&text, &mut cache).ok()
    }

    /// `readLevel`. `present` says which of `.ignore`, `.gitignore`, `.git`
    /// may exist in `dir` (from its listing); a read of one that cannot
    /// exist would fail with `ENOENT`, so it is skipped.
    fn read_level(&self, dir: &str, present: [bool; 3]) -> Level {
        let read = |present: bool, name: &str| {
            if present {
                self.read_ignore_file(&join(dir, name))
            } else {
                None
            }
        };
        Level {
            dir: dir.to_string(),
            kinds: [
                read(present[0], ".ignore"),
                read(present[1], ".gitignore"),
                read(present[2], ".git/info/exclude"),
            ],
        }
    }

    /// `matched` of `files.ts`.
    fn matched(&self, absolute: &str, is_dir: bool, levels: &Levels) -> Result<Match, Error> {
        let mut result = Match::None;
        for (root, matcher, whitelists) in &self.overrides {
            let mut found = test(matcher, candidate(root, absolute)?, is_dir)?;
            if found == Match::None && *whitelists && !is_dir {
                found = Match::Ignore;
            }
            if found == Match::Ignore {
                return Ok(Match::Ignore);
            }
            if found == Match::Whitelist {
                result = Match::Whitelist;
            }
        }
        if result != Match::None {
            return Ok(result);
        }
        for kind in 0..3 {
            for level in levels.iter() {
                if let Some(matcher) = &level.kinds[kind] {
                    let found = test(matcher, candidate(&level.dir, absolute)?, is_dir)?;
                    if found != Match::None {
                        return Ok(found);
                    }
                }
            }
        }
        match &self.global {
            Some(global) => test(global, absolute.trim_start_matches('/'), is_dir),
            None => Ok(Match::None),
        }
    }

    /// `visit(dir, [readLevel(dir), ...levels])` of `files.ts`: `dir`'s own
    /// level is read here, after listing it. Reading ignore files never
    /// fails, so the order is unobservable, and the listing tells which
    /// ignore files can exist: names are matched ASCII-case-insensitively
    /// (and any non-ASCII name counts) so a case-insensitive file system,
    /// where `.GITIGNORE` opens as `.gitignore`, still reads it.
    ///
    /// Returns the selected files beneath `dir`, or the first error in the
    /// sequential walk's order: entries are matched in listing order up to
    /// the first matching error; the subdirectories accepted before it are
    /// walked (possibly in parallel) and their results taken in order, then
    /// that matching error.
    fn visit(&self, dir: &str, parent: Option<Arc<Levels>>) -> Result<Vec<String>, Error> {
        let scandir = |error| Error::Io {
            op: "scandir",
            path: dir.to_string(),
            error,
        };
        // Listed completely first, as the host's `readdir` does, in its
        // order: byte order of the raw names (Node's libuv `scandir` sorts
        // with `strcmp`; the QuickJS host sorts the same way).
        let mut raw = Vec::new();
        for entry in fs::read_dir(dir).map_err(scandir)? {
            let entry = entry.map_err(scandir)?;
            let kind = entry.file_type().map_err(scandir)?;
            raw.push((entry.file_name(), kind.is_dir(), kind.is_file()));
        }
        raw.sort_by(|a, b| a.0.as_encoded_bytes().cmp(b.0.as_encoded_bytes()));
        let mut present = [false; 3];
        let mut entries = Vec::with_capacity(raw.len());
        for (name, is_dir, is_file) in raw {
            let name = name.to_string_lossy().into_owned();
            for (index, special) in [".ignore", ".gitignore", ".git"].iter().enumerate() {
                present[index] |= !name.is_ascii() || name.eq_ignore_ascii_case(special);
            }
            entries.push((name, is_dir, is_file));
        }
        let levels = Arc::new(Levels {
            level: self.read_level(dir, present),
            parent,
        });

        let mut files = Vec::new();
        let mut dirs = Vec::new();
        let mut stopped = None;
        for (name, is_dir, is_file) in entries {
            if !is_dir && !is_file {
                continue; // symlinks and special files
            }
            let absolute = join(dir, &name);
            let accepted = self
                .matched(&absolute, is_dir, &levels)
                .and_then(|found| match found {
                    Match::Ignore => Ok(None),
                    _ if is_dir => Ok(Some(true)),
                    _ => candidate(self.root, &absolute).map(|relative| {
                        files.push(relative.to_string());
                        Some(false)
                    }),
                });
            match accepted {
                Ok(Some(true)) => dirs.push(absolute),
                Ok(_) => {}
                Err(error) => {
                    stopped = Some(error);
                    break;
                }
            }
        }
        for result in self.visit_all(dirs, &levels) {
            files.extend(result?);
        }
        match stopped {
            Some(error) => Err(error),
            None => Ok(files),
        }
    }

    /// `visit` of each directory, results in order. Directories run on
    /// scoped threads while spare ones remain; the rest on this thread.
    fn visit_all(
        &self,
        dirs: Vec<String>,
        levels: &Arc<Levels>,
    ) -> Vec<Result<Vec<String>, Error>> {
        let take = || {
            self.spare
                .fetch_update(Atomic::AcqRel, Atomic::Acquire, |spare| {
                    spare.checked_sub(1)
                })
                .is_ok()
        };
        if dirs.len() < 2 || self.spare.load(Atomic::Acquire) == 0 {
            return dirs
                .iter()
                .map(|dir| self.visit(dir, Some(levels.clone())))
                .collect();
        }
        std::thread::scope(|scope| {
            let started: Vec<_> = dirs
                .iter()
                .map(|dir| {
                    take().then(|| {
                        let levels = levels.clone();
                        scope.spawn(move || {
                            let result = self.visit(dir, Some(levels));
                            self.spare.fetch_add(1, Atomic::AcqRel);
                            result
                        })
                    })
                })
                .collect();
            // This thread's share first, while the others run.
            let here: Vec<_> = dirs
                .iter()
                .zip(&started)
                .map(|(dir, handle)| {
                    handle
                        .is_none()
                        .then(|| self.visit(dir, Some(levels.clone())))
                })
                .collect();
            started
                .into_iter()
                .zip(here)
                .map(|(handle, result)| match (handle, result) {
                    (Some(handle), _) => handle.join().expect("walker thread"),
                    (None, Some(result)) => result,
                    (None, None) => unreachable!("every directory is visited once"),
                })
                .collect()
        })
    }
}

/// `comparePaths` of `files.ts`: component-wise, each component by UTF-16
/// code units (JS string order), a prefix first.
pub fn compare_paths(a: &str, b: &str) -> Ordering {
    let mut left = a.split('/');
    let mut right = b.split('/');
    loop {
        match (left.next(), right.next()) {
            (Some(x), Some(y)) => {
                if x != y {
                    return if x.is_ascii() && y.is_ascii() {
                        x.cmp(y)
                    } else {
                        x.encode_utf16().cmp(y.encode_utf16())
                    };
                }
            }
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

/// The files beneath `request.root` that `files.ts::walk` selects: paths
/// relative to the root, `/`-separated, sorted with [`compare_paths`].
pub fn select(request: &Request) -> Result<Vec<String>, Error> {
    let mut cache = RuleCache::default();
    let mut overrides = Vec::with_capacity(request.overrides.len());
    for set in &request.overrides {
        let matcher = Ignore::from_patterns(set.patterns.iter().map(String::as_str), &mut cache)
            .map_err(Error::Pattern)?;
        overrides.push((set.root.as_str(), matcher, set.whitelists));
    }
    let threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(8);
    let mut walk = Walk {
        root: &request.root,
        overrides,
        global: None,
        cache: Mutex::new(cache),
        spare: AtomicUsize::new(threads - 1),
    };
    walk.global = match &request.global_excludes {
        Some(path) => walk.read_ignore_file(path),
        None => None,
    };
    // Ancestors, farthest last in the chain.
    let mut ancestors = Vec::new();
    let mut dir = dirname(&request.root);
    loop {
        ancestors.push(walk.read_level(dir, [true; 3]));
        if dirname(dir) == dir {
            break;
        }
        dir = dirname(dir);
    }
    let levels = ancestors.into_iter().rev().fold(None, |parent, level| {
        Some(Arc::new(Levels { level, parent }))
    });
    let mut files = walk.visit(&request.root, levels)?;
    files.sort_by(|a, b| compare_paths(a, b));
    Ok(files)
}

/// `readSource` of `jssg.ts` for every path (relative to `root`): the
/// content, or `None` when the file vanished (`ENOENT`) or is not UTF-8
/// (`EILSEQ`). Any other failure is the first error in path order.
pub fn read_sources(root: &str, paths: &[String]) -> Result<Vec<Option<String>>, Error> {
    let read = |path: &String| -> Result<Option<String>, Error> {
        let absolute = join(root, path);
        match fs::read(&absolute) {
            Ok(bytes) => Ok(String::from_utf8(bytes).ok()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Error::Io {
                op: "open",
                path: absolute,
                error,
            }),
        }
    };
    let threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(8);
    if threads <= 1 || paths.len() < 64 {
        return paths.iter().map(read).collect();
    }
    let chunk = paths.len().div_ceil(threads);
    let results: Vec<Vec<Result<Option<String>, Error>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|slice| scope.spawn(move || slice.iter().map(read).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("read thread"))
            .collect()
    });
    results.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_helpers() {
        assert_eq!(dirname("/a/b"), "/a");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(dirname("/"), "/");
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/a", "b"), "/a/b");
        assert_eq!(candidate("/a", "/a/b/c").unwrap(), "b/c");
        assert_eq!(candidate("/", "/a/b").unwrap(), "a/b");
        assert!(candidate("/a/b", "/a/bc/d").is_err());
    }

    #[test]
    fn compare_paths_is_component_wise_utf16() {
        let mut paths = vec!["a-b/c.ts", "a/b.ts", "a/b/c.ts", "\u{1F600}", "\u{FF5E}"];
        paths.sort_by(|a, b| compare_paths(a, b));
        assert_eq!(
            paths,
            vec!["a/b/c.ts", "a/b.ts", "a-b/c.ts", "\u{1F600}", "\u{FF5E}"]
        );
    }
}

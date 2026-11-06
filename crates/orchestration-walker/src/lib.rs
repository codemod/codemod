//! Native file selection for the TypeScript orchestration runtimes.
//!
//! [`select`] is `walk()` of `packages/orchestration/src/execution/files.ts`
//! in Rust, and [`read_sources`] is `readSource` of
//! `src/execution/jssg.ts` over a whole selection. A `Host` that offers them
//! (`Host.selectFiles`, `Host.fs.readSources`; the QuickJS host does) lets
//! `files.ts` skip interpreting the walk. The contract is exact equality with
//! the TypeScript walker, file list and errors, which
//! `packages/orchestration/tests/files-native.test.ts` checks differentially
//! on a fixture tree built to be awkward.
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

struct Walk<'a> {
    root: &'a str,
    overrides: Vec<(&'a str, Ignore, bool)>,
    global: Matcher,
    cache: RuleCache,
    files: Vec<String>,
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
    fn read_ignore_file(&mut self, path: &str) -> Matcher {
        let text = read_lossy(path).ok()?;
        Ignore::from_text(&text, &mut self.cache).ok()
    }

    fn read_level(&mut self, dir: &str) -> Level {
        Level {
            dir: dir.to_string(),
            kinds: [
                self.read_ignore_file(&join(dir, ".ignore")),
                self.read_ignore_file(&join(dir, ".gitignore")),
                self.read_ignore_file(&join(dir, ".git/info/exclude")),
            ],
        }
    }

    /// `matched` of `files.ts`. `levels` is nearest-last.
    fn matched(&self, absolute: &str, is_dir: bool, levels: &[Level]) -> Result<Match, Error> {
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
            for level in levels.iter().rev() {
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

    fn visit(&mut self, dir: &str, levels: &mut Vec<Level>) -> Result<(), Error> {
        let scandir = |error| Error::Io {
            op: "scandir",
            path: dir.to_string(),
            error,
        };
        // Listed completely first, as the host's `readdir` does.
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir).map_err(scandir)? {
            let entry = entry.map_err(scandir)?;
            let kind = entry.file_type().map_err(scandir)?;
            entries.push((
                entry.file_name().to_string_lossy().into_owned(),
                kind.is_dir(),
                kind.is_file(),
            ));
        }
        for (name, is_dir, is_file) in entries {
            if !is_dir && !is_file {
                continue; // symlinks and special files
            }
            let absolute = join(dir, &name);
            if self.matched(&absolute, is_dir, levels)? == Match::Ignore {
                continue;
            }
            if is_dir {
                let level = self.read_level(&absolute);
                levels.push(level);
                let visited = self.visit(&absolute, levels);
                levels.pop();
                visited?;
            } else {
                let relative = candidate(self.root, &absolute)?.to_string();
                self.files.push(relative);
            }
        }
        Ok(())
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
    let mut walk = Walk {
        root: &request.root,
        overrides,
        global: None,
        cache,
        files: Vec::new(),
    };
    walk.global = match &request.global_excludes {
        Some(path) => walk.read_ignore_file(path),
        None => None,
    };
    // Ancestors farthest-first, then the root: nearest-last.
    let mut ancestors = Vec::new();
    let mut dir = dirname(&request.root);
    loop {
        ancestors.push(walk.read_level(dir));
        if dirname(dir) == dir {
            break;
        }
        dir = dirname(dir);
    }
    ancestors.reverse();
    let mut levels = ancestors;
    let root_level = walk.read_level(&request.root);
    levels.push(root_level);
    walk.visit(&request.root, &mut levels)?;
    let mut files = walk.files;
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

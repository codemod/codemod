//! Go modules: `go.mod` and `go.work`. Module paths are kept as written,
//! since Go treats them case-sensitively.
//!
//! - go.mod is the module's own manifest and, since Go 1.17, lists every
//!   module the build needs, so its `require` lines are the module's edges,
//!   each to the version it names. go.mod keeps no edges between the
//!   required modules, so every edge starts at the module itself. go.sum
//!   adds only checksums and is not read.
//! - go.work names the modules of a workspace (`use`). Each one's
//!   requirements resolve to the versions its own go.mod lists, and a
//!   requirement on another module of the workspace resolves to that module.
//!   A go.mod beside a go.work reads as the whole workspace, as the go
//!   command builds it.
//!
//! `replace` with a module points the edge at the replacement module; with
//! a relative directory, at a workspace in that directory, whose go.mod
//! requirements become its edges, resolved to the versions the main modules
//! list.

use std::collections::{HashMap, HashSet};

use super::{ReadFile, UNVERSIONED};
use crate::model::{join, Ecosystem, Graph, Kind};
use crate::semver::compare;

pub fn go_mod(text: &str, read: ReadFile) -> Result<Graph, String> {
    // The go command builds a module beside a go.work as part of it.
    if let Some(work) = read("go.work") {
        return workspace("go.mod", &work, read);
    }
    let module = Directives::parse(text);
    if module.module.is_none() {
        return Err("no module directive".into());
    }
    let replaces = module.replaces.iter().map(|r| (".", r)).collect();
    Ok(build("go.mod", &[(".".into(), &module)], replaces, read))
}

pub fn go_work(text: &str, read: ReadFile) -> Result<Graph, String> {
    workspace("go.work", text, read)
}

fn workspace(lockfile: &str, text: &str, read: ReadFile) -> Result<Graph, String> {
    let work = Directives::parse(text);
    let mut modules = Vec::new();
    for dir in &work.uses {
        let text =
            read(&format!("{dir}/go.mod")).ok_or_else(|| format!("cannot read {dir}/go.mod"))?;
        let module = Directives::parse(&text);
        if module.module.is_none() {
            return Err(format!("{dir}/go.mod: no module directive"));
        }
        modules.push((join(".", dir), module));
    }
    // go.work's own replacements win over the modules'.
    let replaces = work
        .replaces
        .iter()
        .map(|r| (".", r))
        .chain(
            modules
                .iter()
                .flat_map(|(dir, m)| m.replaces.iter().map(move |r| (dir.as_str(), r))),
        )
        .collect();
    let modules: Vec<_> = modules.iter().map(|(dir, m)| (dir.clone(), m)).collect();
    Ok(build(lockfile, &modules, replaces, read))
}

/// The edges of the main modules, and of the directories they are replaced
/// with. `replaces` are (directory they are relative to, replacement).
fn build(
    lockfile: &str,
    modules: &[(String, &Directives)],
    replaces: Vec<(&str, &Replace)>,
    read: ReadFile,
) -> Graph {
    let mut graph = Graph::from_lockfile(lockfile);
    let mut local = HashMap::new();
    let mut mains = Vec::new();
    for (dir, module) in modules {
        let path = module.module.as_deref().unwrap_or_default();
        let id = graph.workspace(Ecosystem::Go, path, UNVERSIONED, dir);
        local.insert(path.to_string(), id);
        mains.push(id);
    }
    // A replaced directory's go.mod lists what it needs, but the build
    // uses the versions the main modules select: the highest they list.
    let mut selected = HashMap::<&str, &str>::new();
    for (path, version) in modules.iter().flat_map(|(_, m)| &m.requires) {
        let entry = selected.entry(path).or_insert(version);
        if compare(Ecosystem::Go, version, entry).is_gt() {
            *entry = version;
        }
    }

    let mut resolver = Resolver {
        local,
        replaces,
        pending: Vec::new(),
        seen: HashSet::new(),
    };
    for ((_, module), from) in modules.iter().zip(mains) {
        for (path, version) in &module.requires {
            let to = resolver.resolve(&mut graph, path, version);
            graph.edge(from, to, path, Some(version.clone()), Kind::Prod);
        }
    }
    while let Some((from, go_mod)) = resolver.pending.pop() {
        let Some(module) = read(&go_mod).map(|text| Directives::parse(&text)) else {
            continue;
        };
        for (path, version) in &module.requires {
            let to = selected
                .get(path.as_str())
                .and_then(|selected| resolver.resolve(&mut graph, path, selected));
            graph.edge(from, to, path, Some(version.clone()), Kind::Prod);
        }
    }
    graph
}

struct Resolver<'a> {
    /// The main modules by path.
    local: HashMap<String, usize>,
    replaces: Vec<(&'a str, &'a Replace)>,
    /// Replaced directories to read: (workspace, its go.mod to read).
    pending: Vec<(usize, String)>,
    seen: HashSet<usize>,
}

impl Resolver<'_> {
    /// The node a requirement on `path` at `version` installs: a main
    /// module, the replacement Go would use, or the module itself.
    fn resolve(&mut self, graph: &mut Graph, path: &str, version: &str) -> Option<usize> {
        if let Some(&id) = self.local.get(path) {
            return Some(id);
        }
        // A replacement for this exact version wins over one for all.
        let replace = self
            .replaces
            .iter()
            .filter(|(_, r)| r.path == path)
            .filter(|(_, r)| r.version.as_deref().is_none_or(|v| v == version))
            .max_by_key(|(_, r)| r.version.is_some());
        let Some((base, replace)) = replace else {
            return Some(graph.node(Ecosystem::Go, path, version));
        };
        match &replace.with {
            Target::Module(module, version) => Some(graph.node(Ecosystem::Go, module, version)),
            Target::Dir(dir) if dir.starts_with("./") || dir.starts_with("../") => {
                let id = graph.workspace(Ecosystem::Go, path, UNVERSIONED, &join(base, dir));
                if self.seen.insert(id) {
                    self.pending.push((id, format!("{base}/{dir}/go.mod")));
                }
                Some(id)
            }
            // An absolute directory is outside the repository.
            Target::Dir(_) => None,
        }
    }
}

struct Replace {
    path: String,
    /// The one version replaced; all of them when absent.
    version: Option<String>,
    with: Target,
}

enum Target {
    Module(String, String),
    Dir(String),
}

/// The directives of a go.mod or go.work that the graph needs.
#[derive(Default)]
struct Directives {
    module: Option<String>,
    requires: Vec<(String, String)>,
    replaces: Vec<Replace>,
    uses: Vec<String>,
}

impl Directives {
    fn parse(text: &str) -> Self {
        let mut out = Self::default();
        let mut block: Option<String> = None;
        for line in text.lines() {
            let tokens = tokens(line);
            let (verb, args) = match (&block, tokens.as_slice()) {
                (_, []) => continue,
                (Some(_), [close]) if close == ")" => {
                    block = None;
                    continue;
                }
                (None, [verb, open]) if open == "(" => {
                    block = Some(verb.clone());
                    continue;
                }
                (Some(verb), args) => (verb.as_str(), args),
                (None, [verb, args @ ..]) => (verb.as_str(), args),
            };
            match (verb, args) {
                ("module", [path, ..]) => out.module = Some(path.clone()),
                ("require", [path, version, ..]) => {
                    out.requires.push((path.clone(), version.clone()));
                }
                ("use", [dir, ..]) => out.uses.push(dir.clone()),
                ("replace", args) => out.replaces.extend(replace(args)),
                _ => {}
            }
        }
        out
    }
}

/// `old [version] => new version` or `old [version] => ./dir`.
fn replace(args: &[String]) -> Option<Replace> {
    let arrow = args.iter().position(|token| token == "=>")?;
    let (old, new) = (&args[..arrow], &args[arrow + 1..]);
    Some(Replace {
        path: old.first()?.clone(),
        version: old.get(1).cloned(),
        with: match new {
            [dir] => Target::Dir(dir.clone()),
            [module, version, ..] => Target::Module(module.clone(), version.clone()),
            [] => return None,
        },
    })
}

/// A line's tokens: words and quoted strings, up to a `//` comment.
fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line.trim_start();
    while !rest.is_empty() && !rest.starts_with("//") {
        let (token, after) = match rest.chars().next() {
            Some(quote @ ('"' | '`')) => {
                let body = &rest[1..];
                let end = body.find(quote).unwrap_or(body.len());
                (&body[..end], body.get(end + 1..).unwrap_or_default())
            }
            _ => {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                rest.split_at(end)
            }
        };
        out.push(token.to_string());
        rest = after.trim_start();
    }
    out
}

//! Dependency patterns, in the spirit of ast-grep patterns for code.
//!
//!   ws:* ~> npm:react-beautiful-dnd              who reaches X, and through which path
//!   ext:* -[peer,prod]-> npm:react refuses 19.0.0  what refuses version V of X
//!   ws:* -> npm:@material-ui/core                 direct dependents
//!   cargo:* -> cargo:syn@^1                        who still pulls syn 1
//!
//! Selectors: `ws:GLOB` (the repo's own packages), `ext:GLOB` (third-party),
//! `npm:GLOB` / `cargo:GLOB` (any package in that ecosystem), each with an
//! optional `@range` on the resolved version. `->` is one edge, `~>` a path.
//! Kinds go in brackets: `-[peer]->`, `~[prod]~>`.

use crate::model::{Eco, Graph, Kind};
use crate::ranges::{breaking_part, cmp, is_prerelease, satisfies};
use crate::registry::Registry;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    Workspace,
    External,
    Eco(Eco),
    Any,
}

#[derive(Debug, Clone)]
pub struct Selector {
    pub scope: Scope,
    pub glob: String,
    pub range: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cond {
    Refuses(String),
    Accepts(String),
}

#[derive(Debug, Clone)]
pub struct Pattern {
    pub from: Selector,
    pub to: Selector,
    pub path: bool,
    pub kinds: Option<Vec<Kind>>,
    pub cond: Option<Cond>,
}

pub fn parse_selector(s: &str) -> Result<Selector, String> {
    let (scope, rest) = match s.split_once(':') {
        Some(("ws", r)) => (Scope::Workspace, r),
        Some(("ext", r)) => (Scope::External, r),
        Some((e, r)) if Eco::parse(e).is_some() => (Scope::Eco(Eco::parse(e).unwrap()), r),
        Some((other, _)) => return Err(format!("unknown selector prefix `{other}:` (use ws:, ext:, npm:, cargo:)")),
        None => (Scope::Any, s),
    };
    // A scoped npm name starts with '@'; a version range starts at the next '@'.
    let (glob, range) = match rest.get(1..).and_then(|r| r.find('@')) {
        Some(i) => (rest[..i + 1].to_string(), Some(rest[i + 2..].to_string())),
        None => (rest.to_string(), None),
    };
    if glob.is_empty() {
        return Err(format!("selector `{s}` has no name (use `*` for any)"));
    }
    Ok(Selector { scope, glob, range })
}

pub fn parse_pattern(src: &str) -> Result<Pattern, String> {
    let t: Vec<&str> = src.split_whitespace().collect();
    if t.len() != 3 && t.len() != 5 {
        return Err("expected `FROM ARROW TO [refuses|accepts VERSION]`".into());
    }
    let arrow = t[1];
    let (path, inner) = if arrow == "->" {
        (false, None)
    } else if arrow == "~>" {
        (true, None)
    } else if let Some(k) = arrow.strip_prefix("-[").and_then(|a| a.strip_suffix("]->")) {
        (false, Some(k))
    } else if let Some(k) = arrow.strip_prefix("~[").and_then(|a| a.strip_suffix("]~>")) {
        (true, Some(k))
    } else {
        return Err(format!("unknown arrow `{arrow}` (use ->, ~>, -[kinds]->, ~[kinds]~>)"));
    };
    let kinds = inner
        .map(|k| k.split(',').map(|k| Kind::parse(k.trim()).ok_or(format!("unknown kind `{k}`"))).collect::<Result<Vec<_>, _>>())
        .transpose()?;
    let cond = match t.get(3) {
        None => None,
        Some(&"refuses") => Some(Cond::Refuses(t[4].into())),
        Some(&"accepts") => Some(Cond::Accepts(t[4].into())),
        Some(w) => return Err(format!("expected `refuses` or `accepts`, got `{w}`")),
    };
    if path && cond.is_some() {
        return Err("refuses/accepts applies to one edge (`->`), not a path (`~>`)".into());
    }
    Ok(Pattern { from: parse_selector(t[0])?, to: parse_selector(t[2])?, path, kinds, cond })
}

fn glob_match(glob: &str, name: &str) -> bool {
    let parts: Vec<&str> = glob.split('*').collect();
    if parts.len() == 1 {
        return glob == name;
    }
    let mut rest = name;
    for (i, p) in parts.iter().enumerate() {
        if i == 0 {
            match rest.strip_prefix(p) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if i == parts.len() - 1 {
            return rest.ends_with(p);
        } else {
            match rest.find(p) {
                Some(j) => rest = &rest[j + p.len()..],
                None => return false,
            }
        }
    }
    true
}

pub fn matches(g: &Graph, sel: &Selector, i: usize) -> bool {
    let n = &g.nodes[i];
    let scope_ok = match sel.scope {
        Scope::Workspace => n.workspace.is_some(),
        Scope::External => n.workspace.is_none(),
        Scope::Eco(e) => n.eco == e,
        Scope::Any => true,
    };
    scope_ok
        && glob_match(&sel.glob, &n.name)
        && sel.range.as_ref().map_or(true, |r| satisfies(n.eco, r, &n.version) == Some(true))
}

#[derive(Debug, Serialize)]
pub struct EdgeHit {
    pub from: String,
    pub workspace: Option<String>,
    pub to: String,
    pub kind: &'static str,
    pub req: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PathHit {
    pub from: String,
    pub workspace: Option<String>,
    pub chain: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Hits {
    Edges(Vec<EdgeHit>),
    Paths(Vec<PathHit>),
}

/// Cargo.lock keeps no requirements for third-party crates, so a `refuses`
/// check fills them from the registry for just the edges it looks at.
pub fn fill_reqs(g: &mut Graph, name_glob: &str, registry: &dyn Registry) {
    let mut cache: HashMap<(Eco, String), Vec<crate::registry::Release>> = HashMap::new();
    for e in g.edges.iter_mut() {
        if e.req.is_some() || !glob_match(name_glob, &e.name) {
            continue;
        }
        let from = &g.nodes[e.from];
        let releases = cache.entry((from.eco, from.name.clone())).or_insert_with(|| registry.releases(from.eco, &from.name));
        if let Some(r) = releases.iter().find(|r| r.version == from.version) {
            if let Some((_, req, kind)) = r.deps.iter().find(|(n, _, k)| *n == e.name && *k != Kind::Dev) {
                e.req = Some(req.clone());
                e.kind = *kind;
            }
        }
    }
}

pub fn run(g: &Graph, p: &Pattern) -> Hits {
    let kind_ok = |k: Kind| p.kinds.as_ref().map_or(true, |ks| ks.contains(&k));
    let to_eco = match p.to.scope {
        Scope::Eco(e) => Some(e),
        _ => None,
    };
    if !p.path {
        let mut hits = vec![];
        for e in &g.edges {
            if !kind_ok(e.kind) || !glob_match(&p.to.glob, &e.name) || !matches(g, &p.from, e.from) {
                continue;
            }
            let eco = g.nodes[e.from].eco;
            if to_eco.is_some_and(|t| t != eco) {
                continue;
            }
            // Resolved targets must match the target selector; peer edges have none, so their range is the test.
            if let Some(t) = e.to {
                if !matches(g, &p.to, t) {
                    continue;
                }
            } else if p.to.range.is_some() {
                continue;
            }
            if let Some(c) = &p.cond {
                let (v, want) = match c {
                    Cond::Refuses(v) => (v, false),
                    Cond::Accepts(v) => (v, true),
                };
                match e.req.as_deref().and_then(|r| satisfies(eco, r, v)) {
                    Some(ok) if ok == want => {}
                    _ => continue,
                }
            }
            hits.push(EdgeHit {
                from: g.label(e.from),
                workspace: g.nodes[e.from].workspace.clone(),
                to: e.to.map(|t| g.label(t)).unwrap_or_else(|| e.name.clone()),
                kind: e.kind.as_str(),
                req: e.req.clone(),
            });
        }
        return Hits::Edges(hits);
    }

    // Paths: shortest chain from each matching start node. A path stops at the
    // repo's other workspaces, so each workspace answers for its own dependencies.
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    for e in &g.edges {
        if let Some(t) = e.to {
            if kind_ok(e.kind) {
                adj.entry(e.from).or_default().push(t);
            }
        }
    }
    let mut hits = vec![];
    for start in (0..g.nodes.len()).filter(|&i| matches(g, &p.from, i)) {
        let mut prev: HashMap<usize, usize> = HashMap::new();
        let mut seen = HashSet::from([start]);
        let mut q = VecDeque::from([start]);
        let mut hit = None;
        'bfs: while let Some(n) = q.pop_front() {
            for &c in adj.get(&n).into_iter().flatten() {
                if !seen.insert(c) {
                    continue;
                }
                prev.insert(c, n);
                if matches(g, &p.to, c) {
                    hit = Some(c);
                    break 'bfs;
                }
                if g.nodes[c].workspace.is_none() {
                    q.push_back(c);
                }
            }
        }
        if let Some(h) = hit {
            let mut chain = vec![g.label(h)];
            let mut x = h;
            while let Some(&p) = prev.get(&x) {
                chain.push(g.label(p));
                x = p;
            }
            chain.reverse();
            hits.push(PathHit { from: g.label(start), workspace: g.nodes[start].workspace.clone(), chain });
        }
    }
    Hits::Paths(hits)
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    /// A later release accepts the version (or drops the dependency).
    Fixable { version: String, breaking: bool },
    /// No release accepts it.
    Stuck { latest: Option<String> },
    /// The installed release accepts it upstream; the refusal comes from the repo's own
    /// overrides, which pnpm writes into the ranges it records.
    LocalOverride,
    Unknown,
}

/// For a package that refuses `dep@target`, find the first later release that accepts it.
pub fn first_release_accepting(registry: &dyn Registry, eco: Eco, pkg: &str, current: &str, dep: &str, target: &str) -> Verdict {
    let all = registry.releases(eco, pkg);
    if all.is_empty() {
        return Verdict::Unknown;
    }
    let accepts = |r: &crate::registry::Release| r.deps.iter().filter(|(n, _, k)| n == dep && *k != Kind::Dev).all(|(_, req, _)| satisfies(eco, req, target) != Some(false));
    if all.iter().find(|r| r.version == current).is_some_and(accepts) {
        return Verdict::LocalOverride;
    }
    let mut releases: Vec<_> = all.into_iter().filter(|r| !r.yanked && !is_prerelease(eco, &r.version)).collect();
    releases.sort_by(|a, b| cmp(eco, &a.version, &b.version));
    let latest = releases.last().map(|r| r.version.clone());
    let ok = releases.iter().filter(|r| cmp(eco, &r.version, current).is_gt()).find(|r| accepts(r));
    match ok {
        Some(r) => Verdict::Fixable { version: r.version.clone(), breaking: breaking_part(&r.version) != breaking_part(current) },
        None => Verdict::Stuck { latest },
    }
}

/// Packages present in more than one version, with who pulls each one.
pub fn duplicates(g: &Graph, eco: Option<Eco>) -> Vec<(String, Vec<(String, usize)>)> {
    let mut by_name: HashMap<(Eco, &str), Vec<usize>> = HashMap::new();
    for (i, n) in g.nodes.iter().enumerate() {
        if n.workspace.is_none() && eco.map_or(true, |e| e == n.eco) {
            by_name.entry((n.eco, &n.name)).or_default().push(i);
        }
    }
    let mut incoming: HashMap<usize, usize> = HashMap::new();
    for e in &g.edges {
        if let Some(t) = e.to {
            *incoming.entry(t).or_default() += 1;
        }
    }
    let mut out: Vec<_> = by_name
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|((e, n), mut v)| {
            v.sort_by(|a, b| cmp(e, &g.nodes[*a].version, &g.nodes[*b].version));
            (format!("{}:{}", e.as_str(), n), v.iter().map(|&i| (g.nodes[i].version.clone(), incoming.get(&i).copied().unwrap_or(0))).collect())
        })
        .collect();
    out.sort();
    out
}

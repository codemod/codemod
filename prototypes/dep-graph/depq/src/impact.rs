//! Variant C: the answer an MCP `dependency_query` tool would return. It joins
//! the graph (who refuses, how it arrives, can a bump fix it) with the code
//! (which files import the package through which the blocker arrives).

use crate::model::{Eco, Graph};
use crate::query::{fill_reqs, first_release_accepting, parse_pattern, run, Hits, Verdict};
use crate::registry::Registry;
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

#[derive(Serialize)]
pub struct Impact {
    pub target: String,
    pub summary: Summary,
    /// The repo's own manifests whose range refuses the target: edit these directly.
    pub own_manifests: Vec<crate::query::EdgeHit>,
    pub blockers: Vec<Blocker>,
}

#[derive(Serialize, Default)]
pub struct Summary {
    pub blockers: usize,
    pub fixable: usize,
    pub fixable_breaking: usize,
    pub stuck: usize,
    pub unknown: usize,
    pub local_override: usize,
    pub workspaces_affected: usize,
    pub files_to_touch: usize,
}

#[derive(Serialize)]
pub struct Blocker {
    pub package: String,
    pub refuses: BTreeMap<String, String>,
    pub fix: Verdict,
    pub workspaces: usize,
    pub direct: usize,
    pub example_chain: Vec<String>,
    /// Packages the repo imports through which this blocker arrives, with the files that import them.
    pub code: Vec<CodeUse>,
}

#[derive(Serialize)]
pub struct CodeUse {
    pub import: String,
    pub files: usize,
    pub sample: Vec<String>,
    pub grep_pattern: String,
}

/// `target` is "eco:name@version"; `also` adds sibling packages bumped together (react-dom with react).
pub fn impact(g: &mut Graph, repo: &Path, target: &str, also: &[String], ignore: &[String], registry: &dyn Registry) -> Result<Impact, String> {
    let sel = crate::query::parse_selector(target)?;
    let eco = match sel.scope {
        crate::query::Scope::Eco(e) => e,
        _ => return Err("target needs an ecosystem prefix, like npm:react@19.0.0".into()),
    };
    let version = sel.range.clone().ok_or("target needs a version, like npm:react@19.0.0")?;
    let names: Vec<String> = std::iter::once(sel.glob.clone()).chain(also.iter().cloned()).collect();

    let mut own = vec![];
    let mut refusers: BTreeMap<String, (String, String, BTreeMap<String, String>)> = BTreeMap::new();
    for name in &names {
        fill_reqs(g, name, registry);
        let p = parse_pattern(&format!("ext:* -[prod,peer,optional]-> {}:{name} refuses {version}", eco.as_str()))?;
        let mine = parse_pattern(&format!("ws:* -[prod,dev,peer,optional,build]-> {}:{name} refuses {version}", eco.as_str()))?;
        if let Hits::Edges(hits) = run(g, &mine) {
            own.extend(hits);
        }
        if let Hits::Edges(hits) = run(g, &p) {
            for h in hits {
                let (pkg, ver) = h.from.rsplit_once('@').unwrap();
                if names.iter().any(|n| n == pkg) || ignore.iter().any(|i| crate::query::parse_selector(i).map(|s| glob(&s.glob, pkg)).unwrap_or(false)) {
                    continue;
                }
                refusers.entry(h.from.clone()).or_insert((pkg.into(), ver.into(), BTreeMap::new())).2.insert(name.clone(), h.req.unwrap_or_default());
            }
        }
    }

    let mut out = Impact { target: target.into(), summary: Summary::default(), own_manifests: own, blockers: vec![] };
    let mut all_ws = HashSet::new();
    let mut all_files = HashSet::new();
    for (label, (pkg, ver, refuses)) in refusers {
        let dep = refuses.keys().next().unwrap().clone();
        let fix = first_release_accepting(registry, eco, &pkg, &ver, &dep, &version);
        match &fix {
            Verdict::Fixable { breaking, .. } => {
                out.summary.fixable += 1;
                out.summary.fixable_breaking += *breaking as usize;
            }
            Verdict::Stuck { .. } => out.summary.stuck += 1,
            Verdict::Unknown => out.summary.unknown += 1,
            Verdict::LocalOverride => out.summary.local_override += 1,
        }
        let p = parse_pattern(&format!("ws:* ~[prod,dev,optional]~> {}:{pkg}@{ver}", eco.as_str()))?;
        let Hits::Paths(paths) = run(g, &p) else { unreachable!() };
        let mut code: BTreeMap<String, (HashSet<String>, Vec<String>)> = BTreeMap::new();
        for ph in &paths {
            all_ws.insert(ph.from.clone());
            let entry = ph.chain[1].rsplit_once('@').map(|(n, _)| n.to_string()).unwrap();
            let dir = ph.workspace.clone().unwrap_or_default();
            let slot = code.entry(entry.clone()).or_default();
            for f in files_importing(repo, &dir, eco, &entry, g) {
                if slot.0.insert(f.clone()) {
                    all_files.insert(f.clone());
                    slot.1.push(f);
                }
            }
        }
        out.blockers.push(Blocker {
            package: label,
            refuses,
            fix,
            workspaces: paths.len(),
            direct: paths.iter().filter(|p| p.chain.len() == 2).count(),
            example_chain: paths.iter().max_by_key(|p| p.chain.len()).map(|p| p.chain.clone()).unwrap_or_default(),
            code: code
                .into_iter()
                .map(|(import, (_, mut files))| {
                    files.sort();
                    CodeUse { grep_pattern: grep_pattern(eco, &import), files: files.len(), sample: files.into_iter().take(3).collect(), import }
                })
                .collect(),
        });
    }
    out.summary.blockers = out.blockers.len();
    out.summary.workspaces_affected = all_ws.len();
    out.summary.files_to_touch = all_files.len();
    Ok(out)
}

fn glob(pat: &str, name: &str) -> bool {
    match pat.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pat == name,
    }
}

fn grep_pattern(eco: Eco, pkg: &str) -> String {
    match eco {
        Eco::Npm => format!("import $$$ from '{pkg}'"),
        Eco::Cargo => format!("use {}::$$$", pkg.replace('-', "_")),
    }
}

/// Source files in one workspace (not in nested workspaces) that import `pkg`.
pub fn files_importing(repo: &Path, ws_dir: &str, eco: Eco, pkg: &str, g: &Graph) -> Vec<String> {
    if ws_dir.is_empty() || ws_dir == "?" {
        return vec![];
    }
    let others: HashSet<&str> = g.nodes.iter().filter_map(|n| n.workspace.as_deref()).filter(|w| *w != ws_dir).collect();
    let (exts, needles): (&[&str], Vec<String>) = match eco {
        Eco::Npm => (&["ts", "tsx", "js", "jsx", "mjs", "cjs"], vec![format!("'{pkg}'"), format!("\"{pkg}\""), format!("'{pkg}/"), format!("\"{pkg}/")]),
        Eco::Cargo => {
            let m = pkg.replace('-', "_");
            (&["rs"], vec![format!("{m}::"), format!("use {m}")])
        }
    };
    let mut out = vec![];
    let mut stack = vec![ws_dir.to_string()];
    while let Some(rel) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(repo.join(&rel)) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let child = if rel == "." { name.clone() } else { format!("{rel}/{name}") };
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if !matches!(name.as_str(), "node_modules" | "target" | ".git" | "dist" | "dist-types" | "build") && !others.contains(child.as_str()) {
                    stack.push(child);
                }
            } else if exts.iter().any(|x| name.ends_with(&format!(".{x}"))) {
                if let Ok(text) = std::fs::read_to_string(e.path()) {
                    let hit = text.lines().any(|l| {
                        let l = l.trim_start();
                        (eco == Eco::Cargo || l.starts_with("import") || l.contains("from ") || l.contains("require(") || l.contains("import("))
                            && needles.iter().any(|n| l.contains(n.as_str()))
                    });
                    if hit {
                        out.push(child);
                    }
                }
            }
        }
    }
    out
}

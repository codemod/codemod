//! Host side of `codemod/deps`: the `semver` host function of the workflow
//! body, and `globalThis.__dependencyHost` in isolates, where
//! `js/dependencies.ts` (`codemod-internal:dependencies`) runs:
//! `registry`, `semver`, and `dependencies()`: the manifests (read and
//! edited through the step's files) and the lockfile graph (loaded on first
//! use, and cached for the process by the content of what it read).

use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use codemod_deps::manifest_edits::{self, Change};
use codemod_deps::project::{self, Project};
use codemod_deps::registry::describe;
use codemod_deps::{semver as versions, Ecosystem, Kind, Registry, Walk};
use codemod_sandbox::sandbox::engine::PendingFiles;
use rquickjs::prelude::Async;
use rquickjs::{Ctx, Function, Object};
use serde_json::{json, Value};

/// Imported by steps and callbacks that use `codemod/deps`; the build
/// points their imports here.
pub const DEPENDENCIES_MODULE: &str = "codemod-internal:dependencies";

pub(crate) fn ecosystem(name: &str) -> Result<Ecosystem, String> {
    Ecosystem::parse(name)
        .ok_or_else(|| format!("unknown ecosystem '{name}'; use npm, cargo or pypi"))
}

/// Each named package's releases, as the JSON `registry.npm(name)` resolves to.
pub(crate) fn releases(
    registry: Option<&dyn Registry>,
    ecosystem_name: &str,
    names: &[String],
    cancel: &AtomicBool,
) -> Result<BTreeMap<String, serde_json::Value>, String> {
    let ecosystem = ecosystem(ecosystem_name)?;
    let registry = registry.ok_or("no package registry is available to this run")?;
    let found = registry.releases(ecosystem, names, cancel)?;
    Ok(names
        .iter()
        .map(|name| {
            let releases = found.get(name).map(Vec::as_slice).unwrap_or_default();
            (name.clone(), describe(ecosystem, name, releases))
        })
        .collect())
}

/// `semver.*` operations; returns JSON.
pub(crate) fn semver(
    op: String,
    ecosystem_name: String,
    a: String,
    b: String,
) -> rquickjs::Result<String> {
    let ecosystem = ecosystem(&ecosystem_name)
        .map_err(|e| rquickjs::Error::new_from_js_message("string", "ecosystem", e))?;
    let value = match op.as_str() {
        "satisfies" => json!(versions::satisfies(ecosystem, &b, &a)),
        "compare" => json!(versions::compare(ecosystem, &a, &b) as i8),
        "breaking" => json!(versions::is_breaking(ecosystem, &a, &b)),
        "prerelease" => json!(versions::is_prerelease(ecosystem, &a)),
        "minVersion" => json!(versions::min_version(ecosystem, &a)),
        "major" => json!(versions::major(ecosystem, &a)),
        "coerce" => json!(versions::coerce(&a)),
        "validRange" => json!(versions::valid_range(ecosystem, &a)),
        "maxSatisfying" => {
            let list: Vec<String> = serde_json::from_str(&a).unwrap_or_default();
            json!(versions::max_satisfying(ecosystem, &list, &b))
        }
        _ => Value::Null,
    };
    Ok(value.to_string())
}

/// What `dependencies()` in one isolate reads and edits through.
struct Files {
    target: PathBuf,
    /// The step's files: what it changed so far, and in a dry run what
    /// earlier steps left.
    pending: Option<PendingFiles>,
}

impl Files {
    fn path(&self, relative: &str) -> PathBuf {
        crate::tasks::changes::under(&self.target, relative)
    }

    fn read(&self, relative: &str) -> Option<String> {
        let path = self.path(relative);
        match &self.pending {
            Some(pending) => pending.read(&path).ok(),
            None => std::fs::read_to_string(path).ok(),
        }
    }

    fn project(&self) -> Project {
        let paths = project::discover(&self.target);
        let present = project::present_root_files(&self.target);
        Project::read(&paths, &present, &|path| self.read(path))
    }
}

/// The project as `dependencies()` sees it, with each declaration resolved.
fn project_json(project: &Project) -> Value {
    let packages: Vec<Value> = project
        .packages
        .iter()
        .map(|package| {
            let declarations: Vec<Value> = package
                .declarations
                .iter()
                .map(|declared| {
                    let resolved = project.resolve(&declared.name, &declared.range);
                    json!({
                        "name": declared.name,
                        "range": declared.range,
                        "kind": declared.kind,
                        "protocol": resolved.protocol,
                        "resolved": resolved.resolved,
                        "catalog": resolved.catalog.map(|(name, file)| json!({ "name": name, "file": file })),
                        "local": resolved.local.map(|i| project.packages[i].dir.clone()),
                        "readable": resolved.readable,
                    })
                })
                .collect();
            json!({
                "dir": package.dir,
                "manifest": package.manifest,
                "name": package.name,
                "version": package.version,
                "declarations": declarations,
            })
        })
        .collect();
    let catalogs: Vec<Value> = project
        .catalogs
        .iter()
        .map(|catalog| {
            let entries: serde_json::Map<String, Value> = catalog
                .entries
                .iter()
                .map(|(name, range)| (name.clone(), json!(range)))
                .collect();
            json!({ "name": catalog.name, "file": catalog.file, "entries": entries })
        })
        .collect();
    json!({
        "packageManager": project.package_manager,
        "packages": packages,
        "catalogs": catalogs,
        "unreadable": project.unreadable,
    })
}

/// Apply manifest changes through the step's files: each file is taken,
/// edited from the text the step has, and set, so the step commits it.
fn edit(files: &Files, changes: &str) -> Result<Value, String> {
    let changes: Vec<Change> =
        serde_json::from_str(changes).map_err(|e| format!("bad dependency edit: {e}"))?;
    let pending = files
        .pending
        .as_ref()
        .ok_or("dependency edits run inside a step")?;
    let project = files.project();
    let mut taken = BTreeMap::new();
    let mut texts = BTreeMap::new();
    for file in manifest_edits::files(&project, &changes) {
        let path = files.path(&file);
        if !path.is_file() && files.read(&file).is_none() {
            continue;
        }
        let (text, guard) = pending
            .take(&path)
            .map_err(|e| format!("cannot read {file}: {e}"))?;
        texts.insert(file.clone(), text);
        taken.insert(file, guard);
    }
    let (out, edits) = manifest_edits::apply(&project, &changes, &texts)?;
    for (file, text) in out {
        if let Some(guard) = taken.get(&file) {
            guard.set(text);
        }
    }
    drop(taken);
    Ok(json!(edits))
}

/// Parsed graphs, by a hash of everything the parse read: kept for the
/// process, so later steps and other workers reuse a parse while its
/// inputs are unchanged.
static GRAPHS: Mutex<Vec<(u64, Arc<Walk>)>> = Mutex::new(Vec::new());
const KEPT_GRAPHS: usize = 8;

fn load_graph(target: &Path, only: &[String]) -> Result<Arc<Walk>, String> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    codemod_deps::PARSER_VERSION.hash(&mut hasher);
    target.hash(&mut hasher);
    only.hash(&mut hasher);
    for input in codemod_deps::lockfiles::inputs(target, only) {
        input.hash(&mut hasher);
        std::fs::read(target.join(&input))
            .unwrap_or_default()
            .hash(&mut hasher);
    }
    let key = hasher.finish();
    let graphs = GRAPHS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, walk)) = graphs.iter().find(|(k, _)| *k == key) {
        return Ok(Arc::clone(walk));
    }
    drop(graphs);
    let walk = Arc::new(Walk::new(codemod_deps::load(target, only)?));
    let mut graphs = GRAPHS.lock().unwrap_or_else(|e| e.into_inner());
    if graphs.len() >= KEPT_GRAPHS {
        graphs.remove(0);
    }
    graphs.push((key, Arc::clone(&walk)));
    Ok(walk)
}

fn node_json(walk: &Walk, id: usize) -> Value {
    let Some(node) = walk.graph.nodes.get(id) else {
        return Value::Null;
    };
    json!({
        "id": id,
        "ecosystem": node.ecosystem,
        "name": node.name,
        "version": node.version,
        "dir": node.workspace,
    })
}

/// Comma-separated kinds; empty means every kind.
fn parse_kinds(text: &str) -> Result<Vec<Kind>, String> {
    text.split(',')
        .filter(|kind| !kind.is_empty())
        .map(|kind| {
            Kind::parse(kind).ok_or_else(|| {
                format!("unknown dependency kind '{kind}'; use prod, dev, peer, optional or build")
            })
        })
        .collect()
}

/// `{ ok: true, value }` or `{ ok: false, error }`, as JSON: the JavaScript
/// side throws the error with its message as it is.
fn answer(result: Result<Value, String>) -> String {
    match result {
        Ok(value) => json!({ "ok": true, "value": value }).to_string(),
        Err(error) => json!({ "ok": false, "error": error }).to_string(),
    }
}

/// Install `__dependencyHost` in an isolate. Lookups stop when `cancel` is
/// set (the run is canceled, or the step runs out of time):
///   releases(ecosystem, namesJson): Promise<string>  one batch of lookups:
///                                    `{ ok, found: { name: releases } }` or `{ ok: false, error }`
///   semver(op, ecosystem, a, b): string
///   project(): string                the manifests, as an answer
///   edit(changesJson): string        manifest changes; the edits, as an answer
///   graph(onlyJson): string          load a graph: `{ id, lockfiles, skipped }`, as an answer
///   node(graph, id), nodes(graph, which, name), edges(graph, direction, key, kinds),
///   chains(graph, fromJson, toJson, kinds): string   over a loaded graph's node ids
pub(crate) fn install(
    ctx: &Ctx<'_>,
    registry: Option<Arc<dyn Registry>>,
    cancel: Arc<AtomicBool>,
    target: PathBuf,
    pending: Option<PendingFiles>,
) -> rquickjs::Result<()> {
    let host = Object::new(ctx.clone())?;
    host.set(
        "releases",
        Function::new(
            ctx.clone(),
            Async(move |ecosystem: String, names: String| {
                let registry = registry.clone();
                let cancel = Arc::clone(&cancel);
                async move {
                    let lookup = tokio::task::spawn_blocking(move || {
                        let names: Vec<String> = serde_json::from_str(&names)
                            .map_err(|e| format!("bad registry lookup: {e}"))?;
                        releases(registry.as_deref(), &ecosystem, &names, &cancel)
                    })
                    .await
                    .unwrap_or_else(|e| Err(format!("the registry lookup stopped: {e}")));
                    let answer = match lookup {
                        Ok(found) => json!({ "ok": true, "found": found }),
                        Err(error) => json!({ "ok": false, "error": error }),
                    };
                    rquickjs::Result::Ok(answer.to_string())
                }
            }),
        )?,
    )?;
    host.set("semver", Function::new(ctx.clone(), semver)?)?;

    let files = Arc::new(Files { target, pending });
    let read = Arc::clone(&files);
    host.set(
        "project",
        Function::new(ctx.clone(), move || {
            answer(Ok(project_json(&read.project())))
        })?,
    )?;
    let edits = Arc::clone(&files);
    host.set(
        "edit",
        Function::new(ctx.clone(), move |changes: String| {
            answer(edit(&edits, &changes))
        })?,
    )?;

    let graphs: Arc<Mutex<Vec<Arc<Walk>>>> = Arc::default();
    let walk_of = {
        let graphs = Arc::clone(&graphs);
        move |id: usize| -> Option<Arc<Walk>> {
            graphs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(id)
                .cloned()
        }
    };
    {
        let graphs = Arc::clone(&graphs);
        let target = files.target.clone();
        host.set(
            "graph",
            Function::new(ctx.clone(), move |only: String| {
                answer((|| {
                    let only: Vec<String> = serde_json::from_str(&only)
                        .map_err(|e| format!("bad lockfiles option: {e}"))?;
                    let walk = load_graph(&target, &only)?;
                    let mut graphs = graphs.lock().unwrap_or_else(|e| e.into_inner());
                    graphs.push(Arc::clone(&walk));
                    Ok(json!({
                        "id": graphs.len() - 1,
                        "lockfiles": walk.graph.lockfiles,
                        "skipped": walk.graph.skipped,
                    }))
                })())
            })?,
        )?;
    }
    {
        let walk_of = walk_of.clone();
        host.set(
            "node",
            Function::new(ctx.clone(), move |graph: usize, id: usize| {
                walk_of(graph)
                    .map(|walk| node_json(&walk, id))
                    .unwrap_or(Value::Null)
                    .to_string()
            })?,
        )?;
    }
    {
        let walk_of = walk_of.clone();
        host.set(
            "nodes",
            Function::new(
                ctx.clone(),
                move |graph: usize, which: String, name: String| {
                    let Some(walk) = walk_of(graph) else {
                        return "[]".to_string();
                    };
                    let ids: Vec<usize> = match which.as_str() {
                        "workspaces" => walk.workspaces(),
                        "named" => walk.nodes_named(&name).to_vec(),
                        _ => (0..walk.graph.nodes.len()).collect(),
                    };
                    Value::Array(ids.into_iter().map(|id| node_json(&walk, id)).collect())
                        .to_string()
                },
            )?,
        )?;
    }
    {
        let walk_of = walk_of.clone();
        host.set(
            "edges",
            Function::new(
                ctx.clone(),
                move |graph: usize, direction: String, key: String, kinds: String| {
                    answer((|| {
                        let kinds = parse_kinds(&kinds)?;
                        let walk = walk_of(graph).ok_or("unknown graph")?;
                        let id = || key.parse::<usize>().unwrap_or(usize::MAX);
                        let edges = match direction.as_str() {
                            "out" if id() < walk.graph.nodes.len() => {
                                walk.dependencies(id(), &kinds)
                            }
                            "in" if id() < walk.graph.nodes.len() => walk.dependents(id(), &kinds),
                            "named" => walk.declaring(&key, &kinds),
                            _ => Vec::new(),
                        };
                        Ok(Value::Array(
                            edges
                                .into_iter()
                                .map(|index| {
                                    let edge = &walk.graph.edges[index];
                                    json!({
                                        "from": edge.from,
                                        "to": edge.to,
                                        "name": edge.name,
                                        "range": edge.range,
                                        "kind": edge.kind,
                                    })
                                })
                                .collect(),
                        ))
                    })())
                },
            )?,
        )?;
    }
    host.set(
        "chains",
        Function::new(
            ctx.clone(),
            move |graph: usize, from: String, to: String, kinds: String| {
                answer((|| {
                    let walk = walk_of(graph).ok_or("unknown graph")?;
                    let ids = |text: &str| -> Vec<usize> {
                        serde_json::from_str::<Vec<usize>>(text)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|&id| id < walk.graph.nodes.len())
                            .collect()
                    };
                    let to: HashSet<usize> = ids(&to).into_iter().collect();
                    Ok(json!(walk.chains(&ids(&from), &to, &parse_kinds(&kinds)?)))
                })())
            },
        )?,
    )?;
    ctx.globals().set("__dependencyHost", host)
}

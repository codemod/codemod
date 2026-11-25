//! Coverage yardstick for the data-flow and call-graph facts, run over a
//! corpus of real repositories. Test-only.
//!
//! ```sh
//! DATAFLOW_CORPUS_DIR=/path/to/repos \
//!   cargo test --release -p language-javascript corpus_metric -- --ignored --nocapture
//! ```
//!
//! Every directory directly under `DATAFLOW_CORPUS_DIR` is one workspace.
//! The report covers:
//!
//! - call sites: resolved to a workspace function, only callbacks found, or
//!   unresolved, with why (what the callee's value comes from);
//! - `flowsFrom` on every call argument: does the trail reach a real source
//!   (literal, object, function, ...), only boundaries (parameter, import,
//!   global, `this`, call), or a dead end (a node with nothing flowing into
//!   it that is neither: a value the facts lost);
//! - syntax the facts builder skips;
//! - parse failures.

use crate::accurate::AccurateAnalyzer;
use crate::dataflow::{CalleeFact, FileFlow};
use crate::flow_query::{call_targets, callback_targets, FlowHost};
use language_core::{FlowEdgeKind, FlowNodeKind};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;

/// Most nodes one argument query visits before it counts as capped.
const ARGUMENT_WALK_LIMIT: usize = 5_000;

#[derive(Default)]
struct Totals {
    files: usize,
    parse_failures: usize,
    failure_messages: BTreeMap<String, (usize, String)>,
    calls: usize,
    resolved: usize,
    callback_only: usize,
    unresolved_why: BTreeMap<&'static str, usize>,
    unresolved_names: HashMap<String, usize>,
    unresolved_samples: HashMap<&'static str, HashMap<String, usize>>,
    arguments: usize,
    arg_real: usize,
    arg_boundary_only: usize,
    arg_dead_end: usize,
    arg_capped: usize,
    boundary_kinds: BTreeMap<&'static str, usize>,
    dead_end_kinds: BTreeMap<String, usize>,
    dead_end_samples: HashMap<String, usize>,
    skipped: BTreeMap<String, usize>,
}

/// Where a value comes from, following only copies (read, assign, alias).
fn roots(flow: &FileFlow, start: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut seen = HashSet::from([start]);
    let mut stack = vec![start];
    while let Some(current) = stack.pop() {
        let sources: Vec<u32> = flow.in_edges[current as usize]
            .iter()
            .filter(|(_, edge)| {
                matches!(
                    edge,
                    FlowEdgeKind::Read | FlowEdgeKind::Assign | FlowEdgeKind::Alias
                )
            })
            .map(|(source, _)| *source)
            .collect();
        if sources.is_empty() {
            out.push(current);
        }
        for source in sources {
            if seen.insert(source) {
                stack.push(source);
            }
        }
    }
    out
}

fn why_unresolved(
    host: &AccurateAnalyzer,
    path: &Path,
    flow: &FileFlow,
    callee: &CalleeFact,
) -> &'static str {
    let (node, member) = match callee {
        CalleeFact::Node(node) => (*node, false),
        CalleeFact::Member { object, .. } => (*object, true),
    };
    let kinds: Vec<FlowNodeKind> = roots(flow, node)
        .into_iter()
        .map(|n| flow.nodes[n as usize].kind)
        .collect();
    let has = |kind: FlowNodeKind| kinds.contains(&kind);
    let text = &flow.nodes[node as usize];
    // An import root: Some(true) when it resolves to a workspace file,
    // Some(false) when it does not.
    let import_state = || -> (bool, bool) {
        let mut resolved = false;
        let mut relative_unresolved = false;
        for n in roots(flow, node) {
            if let Some(i) = flow.imports.get(&n) {
                let name = if i.namespace {
                    "default"
                } else {
                    i.name.as_str()
                };
                if host.resolve_import(path, &i.module, name).is_some() {
                    resolved = true;
                } else if i.module.starts_with('.') {
                    relative_unresolved = true;
                }
            }
        }
        (resolved, relative_unresolved)
    };
    let import_external = || {
        let (resolved, relative) = import_state();
        !resolved && !relative
    };
    let import_lost = || {
        let (resolved, relative) = import_state();
        !resolved && relative
    };
    match member {
        false if has(FlowNodeKind::Global) => {
            if text.name.as_deref() == Some("require") {
                "direct: require"
            } else {
                "direct: global"
            }
        }
        false if has(FlowNodeKind::Parameter) => "direct: parameter",
        false if has(FlowNodeKind::Import) && import_external() => "direct: external import",
        false if has(FlowNodeKind::Import) && import_lost() => {
            "direct: relative import not resolved"
        }
        false if has(FlowNodeKind::Import) => "direct: workspace import, not a function",
        false if has(FlowNodeKind::Call) => "direct: call result",
        false if has(FlowNodeKind::Member) => "direct: property value",
        false if has(FlowNodeKind::This) => "direct: this",
        false if has(FlowNodeKind::Variable) => "direct: variable with no known value",
        false if has(FlowNodeKind::Function) || has(FlowNodeKind::Class) => {
            "direct: function/class not callable here"
        }
        false => "direct: other expression",
        true if has(FlowNodeKind::This) => "method: on this",
        true if has(FlowNodeKind::Global) => "method: on a global",
        true if has(FlowNodeKind::Parameter) => "method: on a parameter",
        true if has(FlowNodeKind::Import) && import_external() => "method: on an external import",
        true if has(FlowNodeKind::Import) && import_lost() => {
            "method: on a relative import not resolved"
        }
        true if has(FlowNodeKind::Import) => "method: on a workspace import",
        true if has(FlowNodeKind::Call) => "method: on a call result",
        true if has(FlowNodeKind::Member) => "method: on a property value",
        true if has(FlowNodeKind::Literal)
            || has(FlowNodeKind::Array)
            || has(FlowNodeKind::Object) =>
        {
            "method: on a literal/array/object"
        }
        true if has(FlowNodeKind::Variable) => "method: on a variable with no known value",
        true if has(FlowNodeKind::Function) || has(FlowNodeKind::Class) => {
            "method: on a function/class (static, call/apply/bind, inherited)"
        }
        true => "method: on another expression",
    }
}

fn boundary(kind: FlowNodeKind) -> Option<&'static str> {
    Some(match kind {
        FlowNodeKind::Parameter => "parameter",
        FlowNodeKind::Import => "import",
        FlowNodeKind::Global => "global",
        FlowNodeKind::This => "this",
        FlowNodeKind::Call => "call",
        _ => return None,
    })
}

fn real_source(kind: FlowNodeKind) -> bool {
    matches!(
        kind,
        FlowNodeKind::Literal
            | FlowNodeKind::Object
            | FlowNodeKind::Array
            | FlowNodeKind::Function
            | FlowNodeKind::Class
            | FlowNodeKind::Jsx
    )
}

/// Backward walk from one argument, as `flowsFrom()` with default options.
fn argument_query(flow: &FileFlow, content: &str, start: u32, totals: &mut Totals) {
    let mut seen = HashSet::from([start]);
    let mut queue = VecDeque::from([start]);
    let (mut real, mut bounded, mut dead) = (false, false, false);
    while let Some(current) = queue.pop_front() {
        if seen.len() > ARGUMENT_WALK_LIMIT {
            totals.arg_capped += 1;
            break;
        }
        let node = &flow.nodes[current as usize];
        let sources: Vec<u32> = flow.in_edges[current as usize]
            .iter()
            .filter(|(_, edge)| *edge != FlowEdgeKind::Return)
            .map(|(source, _)| *source)
            .collect();
        if let Some(kind) = boundary(node.kind) {
            bounded = true;
            *totals.boundary_kinds.entry(kind).or_default() += 1;
        } else if real_source(node.kind) {
            real = true;
        } else if sources.is_empty() {
            dead = true;
            let what = format!(
                "{}{}",
                node.kind.as_str(),
                if node.binding { " (binding)" } else { "" }
            );
            *totals.dead_end_kinds.entry(what.clone()).or_default() += 1;
            let text: String = content
                .get(node.range.start as usize..node.range.end as usize)
                .unwrap_or("")
                .chars()
                .take(40)
                .collect::<String>()
                .replace('\n', " ");
            *totals
                .dead_end_samples
                .entry(format!("{what}: {text}"))
                .or_default() += 1;
        }
        // Real sources (functions, objects) have nothing upstream worth following.
        if real_source(node.kind)
            && node.kind != FlowNodeKind::Object
            && node.kind != FlowNodeKind::Array
        {
            continue;
        }
        for source in sources {
            if seen.insert(source) {
                queue.push_back(source);
            }
        }
    }
    if dead {
        totals.arg_dead_end += 1;
    }
    if real {
        totals.arg_real += 1;
    } else if bounded && !dead {
        totals.arg_boundary_only += 1;
    }
}

fn measure_repo(root: &Path, totals: &mut Totals) {
    let analyzer = AccurateAnalyzer::new(root.to_path_buf());
    let files = analyzer.workspace_files();
    let failures = analyzer.parse_failures();
    totals.parse_failures += failures.len();
    for failure in &failures {
        let message: String = failure.message.chars().take(70).collect();
        let entry = totals
            .failure_messages
            .entry(message)
            .or_insert_with(|| (0, failure.file_path.display().to_string()));
        entry.0 += 1;
    }
    for file in files {
        if file.components().any(|c| c.as_os_str() == "node_modules") {
            continue;
        }
        let (Some(flow), Some(content)) = (analyzer.flow(&file), analyzer.content(&file)) else {
            continue;
        };
        totals.files += 1;
        for (what, count) in &flow.skipped {
            *totals.skipped.entry(what.clone()).or_default() += *count as usize;
        }
        for call in &flow.calls {
            totals.calls += 1;
            if !call_targets(&analyzer, &file, &flow, call).is_empty() {
                totals.resolved += 1;
            } else {
                if !callback_targets(&analyzer, &file, &flow, call).is_empty() {
                    totals.callback_only += 1;
                }
                let why = why_unresolved(&analyzer, &file, &flow, &call.callee);
                *totals.unresolved_why.entry(why).or_default() += 1;
                *totals
                    .unresolved_samples
                    .entry(why)
                    .or_default()
                    .entry(format!(
                        "{}@{}",
                        call.callee_text.chars().take(30).collect::<String>(),
                        file.file_name().unwrap_or_default().to_string_lossy()
                    ))
                    .or_default() += 1;
                *totals
                    .unresolved_names
                    .entry(call.callee_text.chars().take(40).collect())
                    .or_default() += 1;
            }
            for argument in &call.args {
                totals.arguments += 1;
                argument_query(&flow, &content, *argument, totals);
            }
        }
    }
}

fn percent(part: usize, whole: usize) -> String {
    if whole == 0 {
        return "-".to_string();
    }
    format!("{:.1}%", part as f64 * 100.0 / whole as f64)
}

fn top(map: &HashMap<String, usize>, n: usize) -> Vec<(String, usize)> {
    let mut items: Vec<(String, usize)> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
    items.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    items.truncate(n);
    items
}

#[test]
#[ignore]
fn corpus_metric() {
    let Ok(dir) = std::env::var("DATAFLOW_CORPUS_DIR") else {
        return;
    };
    let mut repos: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    repos.sort();
    let mut totals = Totals::default();
    let started = std::time::Instant::now();
    for repo in &repos {
        measure_repo(repo, &mut totals);
    }
    let t = &totals;
    let mut out = String::new();
    use std::fmt::Write;
    let _ = writeln!(
        out,
        "corpus: {} repos, {} files analyzed, {} parse failures ({:?})",
        repos.len(),
        t.files,
        t.parse_failures,
        started.elapsed()
    );
    let _ = writeln!(
        out,
        "calls: {}  resolved {} ({})  unresolved {} ({}), of which with a callback edge {}",
        t.calls,
        t.resolved,
        percent(t.resolved, t.calls),
        t.calls - t.resolved,
        percent(t.calls - t.resolved, t.calls),
        t.callback_only
    );
    let _ = writeln!(out, "unresolved, why:");
    let mut why: Vec<_> = t.unresolved_why.iter().collect();
    why.sort_by(|a, b| b.1.cmp(a.1));
    for (reason, count) in why {
        let _ = writeln!(out, "  {count:>7} {}  {reason}", percent(*count, t.calls));
        if std::env::var("DATAFLOW_CORPUS_SAMPLES").is_ok() {
            let _ = writeln!(out, "      {:?}", top(&t.unresolved_samples[reason], 8));
        }
    }
    let _ = writeln!(
        out,
        "top unresolved callees: {:?}",
        top(&t.unresolved_names, 15)
    );
    let _ = writeln!(out, "arguments: {}  reach a real source {} ({})  boundary only {} ({})  hit a dead end {} ({})  capped {}", t.arguments, t.arg_real, percent(t.arg_real, t.arguments), t.arg_boundary_only, percent(t.arg_boundary_only, t.arguments), t.arg_dead_end, percent(t.arg_dead_end, t.arguments), t.arg_capped);
    let _ = writeln!(out, "boundary steps by kind: {:?}", t.boundary_kinds);
    let _ = writeln!(out, "dead ends by kind: {:?}", t.dead_end_kinds);
    let _ = writeln!(out, "top dead ends: {:#?}", top(&t.dead_end_samples, 25));
    let mut skipped: Vec<_> = t.skipped.iter().collect();
    skipped.sort_by(|a, b| b.1.cmp(a.1));
    let _ = writeln!(out, "skipped syntax: {skipped:?}");
    let _ = writeln!(out, "parse failures: {:?}", t.failure_messages);
    print!("{out}");
    if let Ok(path) = std::env::var("DATAFLOW_CORPUS_OUT") {
        std::fs::write(path, out).unwrap();
    }
}

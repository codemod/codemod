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
use crate::flow_query::{call_targets, callback_targets, functions_from, FlowHost};
use language_core::{CallEdgeKind, FlowEdgeKind, FlowNodeKind};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// Most nodes one argument query visits before it counts as capped.
const ARGUMENT_WALK_LIMIT: usize = 5_000;

#[derive(Default)]
struct Totals {
    files: usize,
    parse_failures: usize,
    failure_messages: BTreeMap<String, (usize, String)>,
    calls: usize,
    resolved: usize,
    /// Resolved only through the TypeScript checker (`via: "type"`).
    resolved_by_type: usize,
    /// Unresolved method calls the checker has a workspace declaration
    /// for, by the declaration's syntax kind: what the type path misses.
    typed_unmapped: BTreeMap<u32, usize>,
    typed_unmapped_samples: HashMap<u32, HashMap<String, usize>>,
    typed_samples: HashMap<String, usize>,
    /// Checker round trips by method: count and time.
    checker_time: Vec<String>,
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
    functions: usize,
    callers_known: usize,
    callers_unknown_why: BTreeMap<&'static str, usize>,
    named_functions: usize,
    named_callers_known: usize,
    named_unknown_why: BTreeMap<&'static str, usize>,
    blocking_names: HashMap<String, usize>,
    /// Sampled argument queries: parameter boundaries reached without and
    /// with `interprocedural`, and the time the interprocedural ones took.
    ip_samples: usize,
    ip_param_boundaries: (usize, usize),
    ip_steps: (usize, usize),
    ip_time: std::time::Duration,
}

/// Globals whose methods are the platform's, never workspace functions:
/// an unresolved `console.log()` does not make a workspace `log` unknown.
const BUILTIN_GLOBALS: &[&str] = &[
    "console", "Math", "JSON", "Object", "Array", "Promise", "Number", "String", "Boolean",
    "Reflect", "Symbol", "Date", "Error", "Intl", "BigInt", "RegExp", "Map", "Set", "WeakMap",
    "WeakSet", "URL", "Atomics", "process", "Buffer",
];

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

/// The name an unresolved call could reach a workspace function by, or None
/// when its callee is the platform's or an external package's.
fn plausible_name(
    host: &AccurateAnalyzer,
    path: &Path,
    flow: &FileFlow,
    call: &crate::dataflow::CallFact,
) -> Option<String> {
    let (node, name) = match &call.callee {
        CalleeFact::Member { object, property } => (*object, property.to_string()),
        CalleeFact::Node(callee) => (
            *callee,
            flow.nodes[*callee as usize].name.as_deref()?.to_string(),
        ),
    };
    let roots = roots(flow, node);
    let outside = roots.iter().all(|&root| {
        let root_node = &flow.nodes[root as usize];
        match root_node.kind {
            FlowNodeKind::Global => root_node
                .name
                .as_deref()
                .is_some_and(|n| BUILTIN_GLOBALS.contains(&n) || n == "require"),
            FlowNodeKind::Import => flow.imports.get(&root).is_some_and(|i| {
                !i.module.starts_with('.')
                    && host
                        .resolve_import(
                            path,
                            &i.module,
                            if i.namespace { "default" } else { &i.name },
                        )
                        .is_none()
                    && host.module_path(path, &i.module).is_none()
            }),
            _ => false,
        }
    });
    (!outside && !roots.is_empty()).then_some(name)
}

/// "Callers known" for one repo: a workspace function counts when
///
/// 1. no unresolved call in the workspace could reach it by name: the
///    unresolved call's method name, or its callee's identifier, equals the
///    function's name (calls on builtin globals and on packages outside the
///    workspace are left out: they cannot be workspace code), and
/// 2. its value never goes where the analysis loses it: an argument of an
///    unresolved call, a return value, a property write, an array element,
///    or a JSX prop (all of these let code we cannot see call it).
///
/// Object literal members and variables are not escapes: a call through
/// them either resolves, or falls under rule 1 by name.
fn measure_callers_known(
    analyzer: &AccurateAnalyzer,
    files: &[(PathBuf, std::sync::Arc<FileFlow>)],
    totals: &mut Totals,
) {
    let mut names: HashSet<String> = HashSet::new();
    // Accessors run on any read or write of their name.
    let mut member_names: HashSet<String> = HashSet::new();
    for (_, flow) in files {
        for node in &flow.nodes {
            if node.kind == FlowNodeKind::Member {
                if let Some(name) = node.name.as_deref() {
                    member_names.insert(name.to_string());
                }
            }
        }
    }
    let mut escaped: HashMap<(PathBuf, u32), &'static str> = HashMap::new();
    let mut escape = |functions: Vec<(PathBuf, u32)>, why: &'static str| {
        for key in functions {
            escaped.entry(key).or_insert(why);
        }
    };
    for (file, flow) in files {
        for call in &flow.calls {
            if !call_targets(analyzer, file, flow, call).is_empty() {
                continue;
            }
            if let Some(name) = plausible_name(analyzer, file, flow, call) {
                names.insert(name);
            }
            for argument in &call.args {
                escape(
                    functions_from(analyzer, file, flow, *argument),
                    "passed to an unresolved call",
                );
            }
        }
        for node in 0..flow.nodes.len() as u32 {
            for &(target, edge) in &flow.out_edges[node as usize] {
                let why = match edge {
                    FlowEdgeKind::Return => "returned",
                    FlowEdgeKind::Store => "stored in a property",
                    FlowEdgeKind::JsxAttribute => "a JSX prop",
                    FlowEdgeKind::Capture
                        if flow.nodes[target as usize].kind == FlowNodeKind::Array =>
                    {
                        "in an array"
                    }
                    _ => continue,
                };
                if matches!(
                    flow.nodes[node as usize].kind,
                    FlowNodeKind::Literal | FlowNodeKind::Call | FlowNodeKind::Member
                ) {
                    continue;
                }
                escape(functions_from(analyzer, file, flow, node), why);
            }
        }
    }
    for (file, flow) in files {
        for (index, function) in flow.functions.iter().enumerate() {
            totals.functions += 1;
            let key = (file.clone(), index as u32);
            let why = if function.accessor.is_some()
                && function
                    .name
                    .as_deref()
                    .is_some_and(|n| member_names.contains(n))
            {
                Some("an accessor, and a property read or write uses its name")
            } else if function.name.as_deref().is_some_and(|n| names.contains(n)) {
                *totals
                    .blocking_names
                    .entry(function.name.as_deref().unwrap_or("").to_string())
                    .or_default() += 1;
                Some("an unresolved call uses its name")
            } else {
                escaped.get(&key).copied()
            };
            let named = function.name.is_some();
            if named {
                totals.named_functions += 1;
            }
            match why {
                None => {
                    totals.callers_known += 1;
                    if named {
                        totals.named_callers_known += 1;
                    }
                }
                Some(why) => {
                    *totals.callers_unknown_why.entry(why).or_default() += 1;
                    if named {
                        *totals.named_unknown_why.entry(why).or_default() += 1;
                    }
                }
            }
        }
    }
}

/// 1-based line and 0-based column (in characters) of a byte offset.
fn line_col(content: &str, offset: u32) -> (usize, usize) {
    let before = &content[..(offset as usize).min(content.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().unwrap_or("").chars().count();
    (line, column)
}

/// Every resolved call edge of a repo, one per line, for comparing with
/// another call graph (Jelly): `file  call-start  call-end  target-file
/// target-end  via`, positions as line:column (1-based line, 0-based column).
fn write_edges(
    analyzer: &AccurateAnalyzer,
    root: &Path,
    files: &[(PathBuf, std::sync::Arc<FileFlow>)],
    out: &Path,
) {
    use std::fmt::Write;
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let relative = |path: &Path| {
        path.strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    let mut text = String::new();
    for (file, flow) in files {
        let Some(content) = analyzer.content(file) else {
            continue;
        };
        for call in &flow.calls {
            let targets = call_targets(analyzer, file, flow, call);
            let callbacks = callback_targets(analyzer, file, flow, call, &targets);
            for (target_path, target, via) in targets.into_iter().chain(callbacks) {
                let (Some(target_flow), Some(target_content)) =
                    (analyzer.flow(&target_path), analyzer.content(&target_path))
                else {
                    continue;
                };
                let function = &target_flow.functions[target as usize];
                let (sl, sc) = line_col(&content, call.range.start);
                let (el, ec) = line_col(&content, call.range.end);
                let (tl, tc) = line_col(&target_content, function.range.end);
                let _ = writeln!(
                    text,
                    "{}\t{sl}:{sc}\t{el}:{ec}\t{}\t{tl}:{tc}\t{}",
                    relative(file),
                    relative(&target_path),
                    via.as_str()
                );
            }
        }
    }
    std::fs::write(out, text).unwrap();
}

/// Every 25th call argument: `flowsFrom()` with and without
/// `interprocedural`, counting the parameters still marked as boundaries.
fn measure_interprocedural(
    analyzer: &AccurateAnalyzer,
    files: &[(PathBuf, std::sync::Arc<FileFlow>)],
    totals: &mut Totals,
) {
    use language_core::FlowOptions;
    // 1 when the answer reaches a real source (literal, object, function…).
    let count = |result: &language_core::FlowResult| {
        usize::from(
            result
                .files
                .iter()
                .flat_map(|f| &f.steps)
                .any(|s| real_source(s.kind)),
        )
    };
    let mut n = 0usize;
    for (file, flow) in files {
        for call in &flow.calls {
            for argument in &call.args {
                n += 1;
                if !n.is_multiple_of(25) {
                    continue;
                }
                let range = flow.nodes[*argument as usize].range;
                let plain =
                    crate::flow_query::data_flow(analyzer, file, range, FlowOptions::default());
                let started = std::time::Instant::now();
                let ip = crate::flow_query::data_flow(
                    analyzer,
                    file,
                    range,
                    FlowOptions {
                        interprocedural: true,
                        ..FlowOptions::default()
                    },
                );
                totals.ip_time += started.elapsed();
                totals.ip_samples += 1;
                totals.ip_param_boundaries.0 += count(&plain);
                totals.ip_param_boundaries.1 += count(&ip);
                totals.ip_steps.0 += plain.files.iter().map(|f| f.steps.len()).sum::<usize>();
                totals.ip_steps.1 += ip.files.iter().map(|f| f.steps.len()).sum::<usize>();
            }
        }
    }
}

fn measure_repo(root: &Path, totals: &mut Totals) {
    let analyzer = AccurateAnalyzer::new(root.to_path_buf());
    // CODEMOD_TS_API set (and not "off"): measure with types on.
    if std::env::var("CODEMOD_TS_API").is_ok_and(|v| !v.is_empty() && v != "off") {
        analyzer.enable_types();
    }
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
    let mut repo_files = Vec::new();
    for file in files {
        if file.components().any(|c| c.as_os_str() == "node_modules") {
            continue;
        }
        if let Some(flow) = analyzer.flow(&file) {
            repo_files.push((file.clone(), flow));
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
            let targets = call_targets(&analyzer, &file, &flow, call);
            if !targets.is_empty() {
                totals.resolved += 1;
                if targets.iter().any(|(_, _, via)| *via == CallEdgeKind::Type) {
                    totals.resolved_by_type += 1;
                    let to: Vec<String> = targets
                        .iter()
                        .filter_map(|(path, function, _)| {
                            let target = analyzer.flow(path)?;
                            let name = target.functions[*function as usize].name.clone();
                            Some(format!(
                                "{}@{}",
                                name.as_deref().unwrap_or("?"),
                                path.file_name()?.to_string_lossy()
                            ))
                        })
                        .collect();
                    *totals
                        .typed_samples
                        .entry(format!(
                            "{} ({}) -> {}",
                            call.callee_text.chars().take(40).collect::<String>(),
                            file.file_name().unwrap_or_default().to_string_lossy(),
                            to.join(", ")
                        ))
                        .or_default() += 1;
                }
            } else {
                if !callback_targets(&analyzer, &file, &flow, call, &[]).is_empty() {
                    totals.callback_only += 1;
                }
                if let (CalleeFact::Member { object, .. }, Some(types)) =
                    (&call.callee, analyzer.types())
                {
                    let end = flow.nodes[*object as usize].range.end;
                    let name = crate::ts_api::skip_trivia(&content, end) as usize;
                    let name = content[name..]
                        .find(|c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == '#')
                        .map(|i| (name + i) as u32);
                    if let Some(name) = name {
                        for decl in types.declarations_at(&file, &content, name) {
                            *totals.typed_unmapped.entry(decl.kind()).or_default() += 1;
                            *totals
                                .typed_unmapped_samples
                                .entry(decl.kind())
                                .or_default()
                                .entry(format!(
                                    "{} -> {}",
                                    call.callee_text.chars().take(40).collect::<String>(),
                                    decl.path.file_name().unwrap_or_default().to_string_lossy()
                                ))
                                .or_default() += 1;
                        }
                    }
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
    measure_callers_known(&analyzer, &repo_files, totals);
    measure_interprocedural(&analyzer, &repo_files, totals);
    if let Ok(out) = std::env::var("DATAFLOW_EDGES_OUT") {
        write_edges(&analyzer, root, &repo_files, Path::new(&out));
    }
    if let Some(types) = analyzer.types() {
        for (method, count, time) in types.stats() {
            totals
                .checker_time
                .push(format!("{method}: {count} in {time:?}"));
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
    if let Ok(only) = std::env::var("DATAFLOW_CORPUS_ONLY") {
        repos.retain(|repo| repo.file_name().is_some_and(|name| name == only.as_str()));
    }
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
    let _ = writeln!(
        out,
        "resolved through types (via: \"type\"): {} ({})",
        t.resolved_by_type,
        percent(t.resolved_by_type, t.calls)
    );
    let _ = writeln!(
        out,
        "unresolved method calls with a workspace declaration, by TS SyntaxKind: {:?}",
        t.typed_unmapped
    );
    if std::env::var("DATAFLOW_CORPUS_SAMPLES").is_ok() {
        let _ = writeln!(
            out,
            "  resolved through types: {:#?}",
            top(&t.typed_samples, 25)
        );
        for (kind, samples) in &t.typed_unmapped_samples {
            let _ = writeln!(out, "  kind {kind}: {:?}", top(samples, 10));
        }
    }
    let _ = writeln!(out, "checker round trips: {:?}", t.checker_time);
    let outside: usize = t
        .unresolved_why
        .iter()
        .filter(|(why, _)| {
            why.contains("external") || why.contains("global") || why.contains("require")
        })
        .map(|(_, count)| *count)
        .sum();
    let _ = writeln!(
        out,
        "calls whose callee is not a global or an external package: {}  resolved {}",
        t.calls - outside,
        percent(t.resolved, t.calls - outside)
    );
    let _ = writeln!(
        out,
        "callers known, named functions (headline): {} of {} ({})",
        t.named_callers_known,
        t.named_functions,
        percent(t.named_callers_known, t.named_functions)
    );
    let mut unknown: Vec<_> = t.named_unknown_why.iter().collect();
    unknown.sort_by(|a, b| b.1.cmp(a.1));
    for (why, count) in unknown {
        let _ = writeln!(
            out,
            "  {count:>7} {}  {why}",
            percent(*count, t.named_functions)
        );
    }
    if std::env::var("DATAFLOW_CORPUS_SAMPLES").is_ok() {
        let _ = writeln!(
            out,
            "  names blocking the most functions: {:?}",
            top(&t.blocking_names, 30)
        );
    }
    let _ = writeln!(
        out,
        "callers known, all functions: {} of {} functions ({})",
        t.callers_known,
        t.functions,
        percent(t.callers_known, t.functions)
    );
    let mut unknown: Vec<_> = t.callers_unknown_why.iter().collect();
    unknown.sort_by(|a, b| b.1.cmp(a.1));
    for (why, count) in unknown {
        let _ = writeln!(out, "  {count:>7} {}  {why}", percent(*count, t.functions));
    }
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
    let _ = writeln!(
        out,
        "interprocedural (every 25th argument, {} queries): reach a real source {} -> {}; steps per answer {:.1} -> {:.1}; {:?} per query",
        t.ip_samples,
        percent(t.ip_param_boundaries.0, t.ip_samples),
        percent(t.ip_param_boundaries.1, t.ip_samples),
        t.ip_steps.0 as f64 / t.ip_samples.max(1) as f64,
        t.ip_steps.1 as f64 / t.ip_samples.max(1) as f64,
        t.ip_time / t.ip_samples.max(1) as u32
    );
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

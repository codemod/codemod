//! Queries over [FileFlow] facts: value flow, mutations and the call graph.
//!
//! Value flow and mutations stay inside one module. The call graph crosses
//! modules: an imported callee is resolved by the host, which is the same
//! import/re-export resolution that answers go-to-definition.

use crate::dataflow::{CallFact, CalleeFact, FileFlow};
use language_core::{
    ByteRange, CallEdgeKind, CallGraphDirection, CallGraphEntry, CallGraphFile, CallGraphOptions,
    CallGraphResult, FileFlowSteps, FlowDirection, FlowEdgeKind, FlowNodeKind, FlowOptions,
    FlowResult, FlowStep, Mutation, MutationKind, MutationOptions, MutationsResult, UnparsedFile,
};
use parking_lot::RwLock;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Most steps or call-graph entries one query returns.
const RESULT_LIMIT: usize = 50_000;

/// What the queries need from an analyzer.
pub(crate) trait FlowHost: Sync {
    /// The path files are cached under.
    fn canonical(&self, path: &Path) -> PathBuf;
    /// Facts for a file, built on first use.
    fn flow(&self, path: &Path) -> Option<Arc<FileFlow>>;
    fn content(&self, path: &Path) -> Option<Arc<str>>;
    /// Where the export export_name of the module specifier (imported
    /// from the file from) is declared: the declaring file and the range of
    /// the declaration's name, or of the export site when it has no name.
    fn resolve_import(
        &self,
        from: &Path,
        specifier: &str,
        export_name: &str,
    ) -> Option<(PathBuf, ByteRange)>;
    /// The file a module specifier (in the file from) resolves to, for
    /// `require()`. None when it does not resolve to a workspace file.
    fn module_path(&self, _from: &Path, _specifier: &str) -> Option<PathBuf> {
        None
    }
    /// Every file a "who calls this" query has to look at.
    fn workspace_files(&self) -> Vec<PathBuf>;
    /// Why the module specifier (imported from the file from) could not be
    /// parsed, when it resolves to a file that does not parse.
    fn import_parse_error(&self, from: &Path, specifier: &str) -> Option<UnparsedFile>;
    /// Workspace files that could not be parsed.
    fn parse_failures(&self) -> Vec<UnparsedFile>;
    /// Where the callers table is kept between queries. None rebuilds it on
    /// every callers query.
    fn caller_index(&self) -> Option<&CallerIndexSlot>;
}

pub(crate) fn data_flow(
    host: &dyn FlowHost,
    path: &Path,
    range: ByteRange,
    options: FlowOptions,
) -> FlowResult {
    let path = host.canonical(path);
    let (Some(flow), Some(content)) = (host.flow(&path), host.content(&path)) else {
        return FlowResult::default();
    };
    let Some(start) = flow.locate(range) else {
        return FlowResult::default();
    };

    let backward = options.direction == FlowDirection::Backward;
    let mut steps = Vec::new();
    let mut truncated = false;
    let mut seen_nodes = HashSet::from([start]);
    let mut seen_steps = HashSet::new();
    let mut queue = VecDeque::from([(start, 0u32)]);

    while let Some((current, depth)) = queue.pop_front() {
        if options.max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        let edges = if backward {
            &flow.in_edges[current as usize]
        } else {
            &flow.out_edges[current as usize]
        };
        for &(other, edge) in edges {
            let call_edge = matches!(edge, FlowEdgeKind::Argument | FlowEdgeKind::Receiver);
            if backward && (edge == FlowEdgeKind::Return || (call_edge && !options.through_calls)) {
                continue;
            }
            // A builtin model's edge goes through a call (`p.then(cb)`).
            if !options.through_calls {
                let pair = if backward {
                    (other, current)
                } else {
                    (current, other)
                };
                if flow.model_edges.contains(&pair) {
                    continue;
                }
            }
            let node = &flow.nodes[other as usize];
            // A call is one step per argument position the value arrives at.
            let index = if edge == FlowEdgeKind::Argument {
                let (call, argument) = if backward {
                    (current, other)
                } else {
                    (other, current)
                };
                flow.call_by_node
                    .get(&call)
                    .and_then(|c| {
                        flow.calls[*c as usize]
                            .args
                            .iter()
                            .position(|a| *a == argument)
                    })
                    .map(|position| position as u32)
            } else {
                node.index
            };
            if seen_steps.insert((other, edge, index)) {
                if steps.len() >= RESULT_LIMIT {
                    truncated = true;
                    break;
                }
                steps.push(FlowStep {
                    range: node.range,
                    kind: node.kind,
                    via: edge,
                    name: node.name.as_deref().map(str::to_string),
                    index,
                    depth: depth + 1,
                    boundary: backward
                        && matches!(
                            node.kind,
                            FlowNodeKind::Parameter
                                | FlowNodeKind::Import
                                | FlowNodeKind::Global
                                | FlowNodeKind::This
                                | FlowNodeKind::Call
                        ),
                });
            }
            // A returned or stored value leaves the function or goes into the
            // heap: reported, not followed.
            let stop = options.stop_at.contains(edge)
                || (!backward
                    && (matches!(edge, FlowEdgeKind::Return | FlowEdgeKind::Store)
                        || (call_edge && !options.through_calls)));
            if !stop && seen_nodes.insert(other) {
                queue.push_back((other, depth + 1));
            }
        }
    }

    FlowResult {
        files: vec![FileFlowSteps {
            file_path: path,
            content: content.to_string(),
            steps,
        }],
        truncated,
    }
}

/// Bindings that may hold the same value as start, start included: the ones
/// its value is copied into, and the ones it was copied from. The two walks
/// are separate, so two sources of one binding are not aliases of each other.
fn aliases(flow: &FileFlow, start: u32) -> HashSet<u32> {
    let mut bindings = HashSet::from([start]);
    for edges in [&flow.out_edges, &flow.in_edges] {
        let mut seen = HashSet::from([start]);
        let mut stack = vec![start];
        while let Some(current) = stack.pop() {
            for &(other, edge) in &edges[current as usize] {
                if !matches!(
                    edge,
                    FlowEdgeKind::Read | FlowEdgeKind::Assign | FlowEdgeKind::Alias
                ) || !seen.insert(other)
                {
                    continue;
                }
                if flow.nodes[other as usize].binding {
                    bindings.insert(other);
                }
                stack.push(other);
            }
        }
    }
    bindings
}

pub(crate) fn mutations(
    host: &dyn FlowHost,
    path: &Path,
    range: ByteRange,
    options: MutationOptions,
) -> MutationsResult {
    let path = host.canonical(path);
    let Some(flow) = host.flow(&path) else {
        return MutationsResult::default();
    };
    let Some(mut binding) = flow.locate(range) else {
        return MutationsResult::default();
    };
    if !flow.nodes[binding as usize].binding {
        let read = flow.in_edges[binding as usize]
            .iter()
            .find(|(_, edge)| *edge == FlowEdgeKind::Read);
        match read {
            Some((declared, _)) => binding = *declared,
            None => return MutationsResult::default(),
        }
    }

    let aliases = aliases(&flow, binding);
    let after_function = options.after.and_then(|after| flow.function_at(after));
    let mut found: Vec<Mutation> = Vec::new();

    for fact in &flow.mutations {
        if !aliases.contains(&fact.binding) {
            continue;
        }
        let via_alias = fact.binding != binding;
        let rebinds = fact.kind == MutationKind::Reassign
            || (fact.kind == MutationKind::Update && fact.name.is_none());
        // Rebinding another name leaves this binding's value alone.
        if via_alias && rebinds {
            continue;
        }
        let home = flow.nodes[fact.binding as usize].function;
        let deferred = fact.function != home;

        if let Some(after) = options.after {
            let later = fact.range.start >= after.end;
            let same_loop = flow
                .loops
                .iter()
                .any(|l| after.is_within(l) && fact.range.is_within(l));
            // A closure can run at any time, unless the point asked about
            // is inside that same closure.
            let any_time = deferred && fact.function != after_function;
            if !(later || same_loop || any_time) {
                continue;
            }
        }
        found.push(Mutation {
            range: fact.range,
            kind: fact.kind,
            confidence: fact.confidence,
            name: fact.name.as_deref().map(str::to_string),
            deferred,
            via_alias,
        });
    }

    found.sort_by_key(|m| (m.range.start, m.range.end));
    MutationsResult { mutations: found }
}

/// A function: its file and index, or None for a module's top-level code.
type FunctionKey = (PathBuf, Option<u32>);

/// Where a callee expression's value comes from.
enum Origin {
    /// A function, class, object literal or call result.
    Node(PathBuf, Arc<FileFlow>, u32),
    /// import * as ns from specifier, seen from a file.
    Namespace(PathBuf, String),
}

/// Follow a value back to the functions, classes and objects it may be,
/// through local aliases and imports.
fn origins(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    start: u32,
    seen: &mut HashSet<(PathBuf, u32)>,
    out: &mut Vec<Origin>,
) {
    let mut stack = vec![start];
    while let Some(current) = stack.pop() {
        if !seen.insert((path.to_path_buf(), current)) {
            continue;
        }
        let node = &flow.nodes[current as usize];
        match node.kind {
            FlowNodeKind::Call => {
                // require("./m") is the module's exports.
                if let Some(specifier) = flow.requires.get(&current) {
                    out.push(Origin::Namespace(path.to_path_buf(), specifier.clone()));
                    commonjs_export(host, path, specifier, None, seen, out);
                } else {
                    out.push(Origin::Node(path.to_path_buf(), Arc::clone(flow), current));
                }
                continue;
            }
            FlowNodeKind::Function | FlowNodeKind::Class | FlowNodeKind::Object => {
                out.push(Origin::Node(path.to_path_buf(), Arc::clone(flow), current));
                continue;
            }
            FlowNodeKind::Import => {
                if let Some(import) = flow.imports.get(&current) {
                    if import.namespace {
                        out.push(Origin::Namespace(path.to_path_buf(), import.module.clone()));
                    } else if let Some((target, range)) =
                        host.resolve_import(path, &import.module, &import.name)
                    {
                        origins_at(host, &target, range, seen, out);
                    } else {
                        // A CommonJS module imported with `import`.
                        let name = (import.name != "default").then_some(import.name.as_str());
                        commonjs_export(host, path, &import.module, name, seen, out);
                    }
                }
                continue;
            }
            _ => {}
        }
        // `await import("./m")`, `import("./m").then((ns) => …)`: the
        // module's namespace (the promise of it stands for it too:
        // `.then(({ f }) => …)` destructures it).
        if let Some(specifier) = flow.dynamic_imports.get(&current) {
            out.push(Origin::Namespace(path.to_path_buf(), specifier.clone()));
            continue;
        }
        for &(source, edge) in &flow.in_edges[current as usize] {
            if edge == FlowEdgeKind::Derive {
                if let Some(specifier) = flow.dynamic_imports.get(&source) {
                    out.push(Origin::Namespace(path.to_path_buf(), specifier.clone()));
                }
            }
        }
        // `obj.f`, `ns.f`, `require("./m").f`, `const { f } = obj`: the
        // property of whatever the object may be.
        for &(object, edge) in &flow.in_edges[current as usize] {
            if edge != FlowEdgeKind::Property {
                continue;
            }
            let name = match node.kind {
                FlowNodeKind::Member => node.name.as_deref(),
                _ => flow
                    .property_keys
                    .get(&(object, current))
                    .map(String::as_str),
            };
            let Some(name) = name else {
                continue;
            };
            let mut bases = Vec::new();
            origins(host, path, flow, object, seen, &mut bases);
            for base in &bases {
                member_origins(host, base, name, seen, out);
            }
        }
        for &(source, edge) in &flow.in_edges[current as usize] {
            if matches!(
                edge,
                FlowEdgeKind::Read | FlowEdgeKind::Assign | FlowEdgeKind::Alias
            ) {
                stack.push(source);
            }
        }
    }
}

/// What a CommonJS module exports: `module.exports` as a whole (name
/// None), or one property of it (`exports.f = …`, `module.exports = { f }`).
fn commonjs_export(
    host: &dyn FlowHost,
    from: &Path,
    specifier: &str,
    name: Option<&str>,
    seen: &mut HashSet<(PathBuf, u32)>,
    out: &mut Vec<Origin>,
) {
    let Some(target) = host.module_path(from, specifier) else {
        return;
    };
    let Some(flow) = host.flow(&target) else {
        return;
    };
    let mut whole = Vec::new();
    for (export, value) in &flow.commonjs_exports {
        match (name, export) {
            (_, None) => whole.push(*value),
            (Some(name), Some(export)) if name == export => {
                origins(host, &target, &flow, *value, seen, out);
            }
            _ => {}
        }
    }
    match name {
        None => {
            for value in whole {
                origins(host, &target, &flow, value, seen, out);
            }
        }
        Some(name) => {
            let mut bases = Vec::new();
            for value in whole {
                origins(host, &target, &flow, value, seen, &mut bases);
            }
            for base in &bases {
                member_origins(host, base, name, seen, out);
            }
        }
    }
}

/// Origins of a declaration in another file: its binding, or, for an export
/// site with no name (export default () => {}), the values written there.
fn origins_at(
    host: &dyn FlowHost,
    path: &Path,
    range: ByteRange,
    seen: &mut HashSet<(PathBuf, u32)>,
    out: &mut Vec<Origin>,
) {
    let Some(flow) = host.flow(path) else {
        return;
    };
    if let Some(node) = flow.exact(range) {
        origins(host, path, &flow, node, seen, out);
        return;
    }
    let inside: Vec<(u32, ByteRange)> = flow
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            n.range.is_within(&range)
                && matches!(
                    n.kind,
                    FlowNodeKind::Function | FlowNodeKind::Class | FlowNodeKind::Object
                )
        })
        .map(|(index, n)| (index as u32, n.range))
        .collect();
    for (index, node_range) in &inside {
        let nested = inside
            .iter()
            .any(|(other, outer)| other != index && node_range.is_within(outer));
        if !nested {
            origins(host, path, &flow, *index, seen, out);
        }
    }
}

fn method_of(flow: &FileFlow, class_node: u32, name: &str) -> Option<u32> {
    flow.class_methods
        .get(&class_node)?
        .iter()
        .find(|(method, _)| &**method == name)
        .map(|(_, function)| *function)
}

/// The values stored in a class field by its initializer.
fn field_values(flow: &FileFlow, class_node: u32, name: &str) -> Vec<u32> {
    flow.class_fields
        .get(&class_node)
        .into_iter()
        .flatten()
        .filter(|(field, _)| &**field == name)
        .flat_map(|(_, node)| {
            flow.in_edges[*node as usize]
                .iter()
                .filter(|(_, edge)| *edge == FlowEdgeKind::Store)
                .map(|(value, _)| *value)
        })
        .collect()
}

/// The functions an origin stands for when it is called.
fn functions_of(origin: &Origin, out: &mut Vec<(PathBuf, u32)>) {
    let Origin::Node(path, flow, node) = origin else {
        return;
    };
    match flow.nodes[*node as usize].kind {
        FlowNodeKind::Function => {
            if let Some(function) = flow.fn_by_node.get(node) {
                out.push((path.clone(), *function));
            }
        }
        FlowNodeKind::Class => {
            if let Some(function) = method_of(flow, *node, "constructor") {
                out.push((path.clone(), function));
            }
        }
        _ => {}
    }
}

/// The values origin.property may be: a namespace or module export, an
/// object literal member, a class's method or static field, or the method
/// of an instance made with `new`.
fn member_origins(
    host: &dyn FlowHost,
    origin: &Origin,
    property: &str,
    seen: &mut HashSet<(PathBuf, u32)>,
    out: &mut Vec<Origin>,
) {
    match origin {
        Origin::Namespace(from, specifier) => {
            if let Some((target, range)) = host.resolve_import(from, specifier, property) {
                origins_at(host, &target, range, seen, out);
            } else {
                commonjs_export(host, from, specifier, Some(property), seen, out);
            }
        }
        Origin::Node(path, flow, node) => match flow.nodes[*node as usize].kind {
            FlowNodeKind::Object => {
                let member = flow
                    .object_members
                    .get(node)
                    .and_then(|members| members.iter().find(|(name, _)| &**name == property));
                if let Some((_, value)) = member {
                    origins(host, path, flow, *value, seen, out);
                }
            }
            FlowNodeKind::Class => class_member(path, flow, *node, property, host, seen, out),
            // new C().method()
            FlowNodeKind::Call => {
                let constructed = flow
                    .call_by_node
                    .get(node)
                    .map(|call| &flow.calls[*call as usize])
                    .filter(|call| call.is_new);
                if let Some(CallFact {
                    callee: CalleeFact::Node(callee),
                    ..
                }) = constructed
                {
                    let mut classes = Vec::new();
                    origins(host, path, flow, *callee, seen, &mut classes);
                    for class in &classes {
                        if let Origin::Node(class_path, class_flow, class_node) = class {
                            if class_flow.nodes[*class_node as usize].kind == FlowNodeKind::Class {
                                class_member(
                                    class_path,
                                    class_flow,
                                    *class_node,
                                    property,
                                    host,
                                    seen,
                                    out,
                                );
                            }
                        }
                    }
                }
            }
            _ => {}
        },
    }
}

/// A class's method, or the value of its field.
fn class_member(
    path: &Path,
    flow: &Arc<FileFlow>,
    class_node: u32,
    property: &str,
    host: &dyn FlowHost,
    seen: &mut HashSet<(PathBuf, u32)>,
    out: &mut Vec<Origin>,
) {
    if let Some(function) = method_of(flow, class_node, property) {
        let node = flow.functions[function as usize].node;
        out.push(Origin::Node(path.to_path_buf(), Arc::clone(flow), node));
        return;
    }
    for value in field_values(flow, class_node, property) {
        origins(host, path, flow, value, seen, out);
    }
}

/// The functions origin.property(…) may call.
fn member_functions(
    host: &dyn FlowHost,
    origin: &Origin,
    property: &str,
    out: &mut Vec<(PathBuf, u32)>,
) {
    let mut found = Vec::new();
    member_origins(host, origin, property, &mut HashSet::new(), &mut found);
    for origin in &found {
        functions_of(origin, out);
    }
}

/// The workspace functions a call may call.
pub(crate) fn call_targets(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    call: &CallFact,
) -> Vec<(PathBuf, u32, CallEdgeKind)> {
    let mut functions = Vec::new();
    let mut seen = HashSet::new();
    let via = match &call.callee {
        CalleeFact::Member { object, property } => {
            if flow.nodes[*object as usize].kind == FlowNodeKind::This {
                let class = flow.class_of(call.function);
                let method = class.and_then(|class| method_of(flow, class, property));
                if let Some(function) = method {
                    functions.push((path.to_path_buf(), function));
                } else if let Some(class) = class {
                    // `run = bound(fn)`, `handler = existing`: a field whose
                    // value is a function.
                    let mut found = Vec::new();
                    for value in field_values(flow, class, property) {
                        origins(host, path, flow, value, &mut seen, &mut found);
                    }
                    for origin in &found {
                        functions_of(origin, &mut functions);
                    }
                }
            } else {
                let mut found = Vec::new();
                origins(host, path, flow, *object, &mut seen, &mut found);
                for origin in &found {
                    member_functions(host, origin, property, &mut functions);
                }
            }
            if call.is_jsx {
                CallEdgeKind::Jsx
            } else {
                CallEdgeKind::Method
            }
        }
        CalleeFact::Node(callee) => {
            let mut found = Vec::new();
            origins(host, path, flow, *callee, &mut seen, &mut found);
            for origin in &found {
                functions_of(origin, &mut functions);
            }
            if call.is_new {
                CallEdgeKind::Constructor
            } else if call.is_jsx {
                CallEdgeKind::Jsx
            } else {
                CallEdgeKind::Direct
            }
        }
    };
    functions.dedup();
    functions
        .into_iter()
        .map(|(path, function)| (path, function, via))
        .collect()
}

/// Functions handed to a call the analysis cannot see into. They are
/// assumed to be called: items.map(f), setTimeout(f), app.get("/", f).
pub(crate) fn callback_targets(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    call: &CallFact,
) -> Vec<(PathBuf, u32, CallEdgeKind)> {
    let mut functions = Vec::new();
    let mut seen = HashSet::new();
    for argument in &call.args {
        let mut found = Vec::new();
        origins(host, path, flow, *argument, &mut seen, &mut found);
        for origin in &found {
            if let Origin::Node(_, origin_flow, node) = origin {
                if origin_flow.nodes[*node as usize].kind == FlowNodeKind::Function {
                    functions_of(origin, &mut functions);
                }
            }
        }
    }
    functions
        .into_iter()
        .map(|(path, function)| (path, function, CallEdgeKind::Callback))
        .collect()
}

fn unresolved_kind(call: &CallFact) -> CallEdgeKind {
    match &call.callee {
        CalleeFact::Member { .. } if call.is_jsx => CallEdgeKind::Jsx,
        CalleeFact::Member { .. } => CallEdgeKind::Method,
        CalleeFact::Node(_) if call.is_new => CallEdgeKind::Constructor,
        CalleeFact::Node(_) if call.is_jsx => CallEdgeKind::Jsx,
        CalleeFact::Node(_) => CallEdgeKind::Direct,
    }
}

/// The functions a syntax range stands for: a function or its name, a call
/// (its callees), a reference to a function, or, with outermost, the
/// functions declared inside it.
fn functions_at(
    host: &dyn FlowHost,
    path: &Path,
    range: ByteRange,
    outermost: bool,
) -> Vec<FunctionKey> {
    let Some(flow) = host.flow(path) else {
        return Vec::new();
    };
    let named: Vec<FunctionKey> = flow
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.range == range || f.name_range == Some(range))
        .map(|(index, _)| (path.to_path_buf(), Some(index as u32)))
        .collect();
    if !named.is_empty() {
        return named;
    }

    if let Some(node) = flow.exact(range) {
        let mut functions = Vec::new();
        if let Some(call) = flow.call_by_node.get(&node) {
            for (target, function, _) in
                call_targets(host, path, &flow, &flow.calls[*call as usize])
            {
                functions.push((target, function));
            }
        } else {
            let mut found = Vec::new();
            origins(host, path, &flow, node, &mut HashSet::new(), &mut found);
            for origin in &found {
                functions_of(origin, &mut functions);
            }
        }
        if !functions.is_empty() {
            return functions
                .into_iter()
                .map(|(path, function)| (path, Some(function)))
                .collect();
        }
    }

    // export const f = () => {} asked about as a whole statement.
    if !outermost {
        return Vec::new();
    }
    outermost_functions(&flow, path, range)
}

/// The functions inside range that are not inside another one of them.
fn outermost_functions(flow: &FileFlow, path: &Path, range: ByteRange) -> Vec<FunctionKey> {
    let inside: Vec<(usize, ByteRange)> = flow
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.range.is_within(&range))
        .map(|(index, f)| (index, f.range))
        .collect();
    inside
        .iter()
        .filter(|(index, inner)| {
            !inside
                .iter()
                .any(|(other, outer)| other != index && inner.is_within(outer))
        })
        .map(|(index, _)| (path.to_path_buf(), Some(*index as u32)))
        .collect()
}

/// Call-graph entries, grouped by file as they are found.
#[derive(Default)]
struct Entries {
    files: Vec<(PathBuf, Vec<CallGraphEntry>)>,
    count: usize,
}

impl Entries {
    fn push(&mut self, path: &Path, entry: CallGraphEntry) -> bool {
        if self.count >= RESULT_LIMIT {
            return false;
        }
        self.count += 1;
        match self.files.iter_mut().find(|(file, _)| file == path) {
            Some((_, entries)) => entries.push(entry),
            None => self.files.push((path.to_path_buf(), vec![entry])),
        }
        true
    }

    /// The result, with each file's content from content (the content the
    /// entries' ranges point into).
    fn finish(
        self,
        content: impl Fn(&Path) -> Option<Arc<str>>,
        truncated: bool,
        unparsed: Vec<UnparsedFile>,
    ) -> CallGraphResult {
        CallGraphResult {
            files: self
                .files
                .into_iter()
                .filter_map(|(file_path, entries)| {
                    Some(CallGraphFile {
                        content: content(&file_path)?.to_string(),
                        file_path,
                        entries,
                    })
                })
                .collect(),
            truncated,
            unparsed,
        }
    }
}

pub(crate) fn call_graph(
    host: &dyn FlowHost,
    path: &Path,
    range: ByteRange,
    options: CallGraphOptions,
) -> CallGraphResult {
    let path = host.canonical(path);
    match options.direction {
        CallGraphDirection::Callees => {
            // The whole module: the calls made while it is evaluated.
            if is_whole_module(host, &path, range) {
                return callees(host, vec![(path, None)], None, options);
            }
            // A call or JSX element: what it calls.
            if let Some((function, call)) = call_at(host, &path, range) {
                return callees(
                    host,
                    vec![(path, function)],
                    Some(FirstStep::Call(call)),
                    options,
                );
            }
            let start = functions_at(host, &path, range, false);
            if !start.is_empty() {
                return callees(host, start, None, options);
            }
            let Some(flow) = host.flow(&path) else {
                return CallGraphResult::default();
            };
            let enclosing = flow.function_at(range);
            let calls_inside = flow
                .calls
                .iter()
                .any(|call| call.function == enclosing && call.range.is_within(&range));
            // `export const f = () => {}` as a whole statement: f.
            if !calls_inside {
                let declared = outermost_functions(&flow, &path, range);
                if !declared.is_empty() {
                    return callees(host, declared, None, options);
                }
            }
            // Any other code (`<div>…</div>`, a block, a statement): the
            // calls written inside it.
            callees(
                host,
                vec![(path, enclosing)],
                Some(FirstStep::Within(range)),
                options,
            )
        }
        CallGraphDirection::Callers => {
            let mut start = functions_at(host, &path, range, true);
            if start.is_empty() {
                start = host
                    .flow(&path)
                    .and_then(|flow| flow.function_at(range))
                    .map(|function| vec![(path.clone(), Some(function))])
                    .unwrap_or_default();
            }
            if start.is_empty() {
                return CallGraphResult::default();
            }
            callers(host, start, options)
        }
    }
}

/// Where a callees walk starts inside its first function.
#[derive(Clone, Copy)]
enum FirstStep {
    /// Only this call.
    Call(u32),
    /// Only the calls inside this range.
    Within(ByteRange),
}

/// True when range covers all of the file's code (the program node).
fn is_whole_module(host: &dyn FlowHost, path: &Path, range: ByteRange) -> bool {
    let Some(content) = host.content(path) else {
        return false;
    };
    let first = content.len() - content.trim_start().len();
    let last = content.trim_end().len();
    last > first && range.start as usize <= first && range.end as usize >= last
}

/// The call at range, with the function it is made from: the call or `new`
/// expression itself, or for JSX the element, its opening tag or its name.
fn call_at(host: &dyn FlowHost, path: &Path, range: ByteRange) -> Option<(Option<u32>, u32)> {
    let flow = host.flow(path)?;
    let found = flow
        .exact(range)
        .and_then(|node| flow.call_by_node.get(&node).copied())
        .or_else(|| {
            flow.calls
                .iter()
                .position(|call| {
                    // The opening tag starts with the element; the tag name
                    // right after its `<`.
                    call.is_jsx
                        && range.is_within(&call.range)
                        && (range.start == call.range.start || range.start == call.range.start + 1)
                })
                .map(|index| index as u32)
        })?;
    Some((flow.calls[found as usize].function, found))
}

/// Walk from start. With first, the first step takes only some calls of the
/// start function instead of all of them.
fn callees(
    host: &dyn FlowHost,
    start: Vec<FunctionKey>,
    first: Option<FirstStep>,
    options: CallGraphOptions,
) -> CallGraphResult {
    let mut entries = Entries::default();
    let mut truncated = false;
    let mut visited: HashSet<FunctionKey> = if first.is_some() {
        HashSet::new()
    } else {
        start.iter().cloned().collect()
    };
    let mut reported: HashSet<FunctionKey> = HashSet::new();
    let mut files_seen: HashSet<PathBuf> = HashSet::new();
    let mut unparsed: Vec<UnparsedFile> = Vec::new();
    let mut queue: VecDeque<(FunctionKey, u32)> = start.into_iter().map(|key| (key, 0)).collect();

    'walk: while let Some(((path, function), depth)) = queue.pop_front() {
        if options.max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        let Some(flow) = host.flow(&path) else {
            continue;
        };
        // An import that does not parse hides the callees in it.
        if files_seen.insert(path.clone()) {
            let mut modules: Vec<&String> = flow.imports.values().map(|i| &i.module).collect();
            modules.sort();
            modules.dedup();
            for module in modules {
                if let Some(failure) = host.import_parse_error(&path, module) {
                    if !unparsed.iter().any(|u| u.file_path == failure.file_path) {
                        unparsed.push(failure);
                    }
                }
            }
        }
        let first_step = if depth == 0 { first } else { None };
        for (index, call) in flow.calls.iter().enumerate() {
            let wanted = match first_step {
                None => true,
                Some(FirstStep::Call(only)) => index as u32 == only,
                Some(FirstStep::Within(range)) => call.range.is_within(&range),
            };
            if call.function != function || !wanted {
                continue;
            }
            let mut targets = call_targets(host, &path, &flow, call);
            if targets.is_empty() {
                let unresolved = CallGraphEntry {
                    range: call.range,
                    name: Some(call.callee_text.to_string()),
                    resolved: false,
                    via: unresolved_kind(call),
                    depth: depth + 1,
                    call_site: Some((path.clone(), call.range)),
                };
                if !entries.push(&path, unresolved) {
                    truncated = true;
                    break 'walk;
                }
                targets = callback_targets(host, &path, &flow, call);
            }
            for (target_path, target, via) in targets {
                let key = (target_path.clone(), Some(target));
                if !reported.insert(key.clone()) {
                    continue;
                }
                let Some(target_flow) = host.flow(&target_path) else {
                    continue;
                };
                let fact = &target_flow.functions[target as usize];
                let entry = CallGraphEntry {
                    range: fact.range,
                    name: fact.name.as_deref().map(str::to_string),
                    resolved: true,
                    via,
                    depth: depth + 1,
                    call_site: Some((path.clone(), call.range)),
                };
                if !entries.push(&target_path, entry) {
                    truncated = true;
                    break 'walk;
                }
                if options.transitive
                    && !options.stop_at.contains(via)
                    && visited.insert(key.clone())
                {
                    queue.push_back((key, depth + 1));
                }
            }
        }
    }
    entries.finish(|file| host.content(file), truncated, unparsed)
}

/// Every call in the workspace, by the function it may call, with the facts
/// and content it was built from. Kept between queries (see
/// [CallerIndexSlot]), so callers answers are a snapshot of the workspace as
/// it was when the first callers query ran.
pub(crate) struct CallerIndex {
    flows: HashMap<PathBuf, Arc<FileFlow>>,
    contents: HashMap<PathBuf, Arc<str>>,
    /// Calling file (index into `paths`), call index, how.
    calls_of: HashMap<FunctionKey, Vec<(u32, u32, CallEdgeKind)>>,
    /// The files with calls, in path order.
    paths: Vec<PathBuf>,
    unparsed: Vec<UnparsedFile>,
}

impl CallerIndex {
    fn build(host: &dyn FlowHost) -> Self {
        let mut index = CallerIndex {
            flows: HashMap::new(),
            contents: HashMap::new(),
            calls_of: HashMap::new(),
            paths: Vec::new(),
            unparsed: Vec::new(),
        };
        let mut files = host.workspace_files();
        files.sort();
        // Each file's calls are resolved on their own, in parallel; the
        // table is then put together in file order.
        let contributions: Vec<_> = files
            .into_par_iter()
            .filter_map(|file| {
                let (Some(flow), Some(content)) = (host.flow(&file), host.content(&file)) else {
                    return None;
                };
                let calls = file_calls(host, &file, &flow);
                Some((file, flow, content, calls))
            })
            .collect();
        for (file, flow, content, calls) in contributions {
            let file_id = index.paths.len() as u32;
            for (target, call_index, via) in calls {
                index
                    .calls_of
                    .entry(target)
                    .or_default()
                    .push((file_id, call_index, via));
            }
            index.paths.push(file.clone());
            index.flows.insert(file.clone(), flow);
            index.contents.insert(file, content);
        }
        index.unparsed = host.parse_failures();
        index
    }

    /// The snapshot's key for a function found in its file's current facts:
    /// the same function when the file is unchanged, otherwise the one with
    /// the same range and name, if the snapshot has one.
    fn key_for(&self, host: &dyn FlowHost, key: &FunctionKey) -> Option<FunctionKey> {
        let (path, Some(function)) = key else {
            return Some(key.clone());
        };
        // A file the walk did not cover (outside the workspace root): its
        // calls are keyed by the facts it had then, which are all there is.
        let Some(snapshot) = self.flows.get(path) else {
            return Some(key.clone());
        };
        let current = host.flow(path)?;
        if Arc::ptr_eq(snapshot, &current) {
            return Some(key.clone());
        }
        let fact = current.functions.get(*function as usize)?;
        snapshot
            .functions
            .iter()
            .position(|f| f.range == fact.range && f.name == fact.name)
            .map(|index| (path.clone(), Some(index as u32)))
    }
}

/// One file's contribution to the callers table: each call's targets, as
/// (target function, call index, how). Depends only on the file's facts and
/// what its imports resolve to, so a changed file's part can be rebuilt alone.
fn file_calls(
    host: &dyn FlowHost,
    file: &Path,
    flow: &Arc<FileFlow>,
) -> Vec<(FunctionKey, u32, CallEdgeKind)> {
    let mut calls = Vec::new();
    for (call_index, call) in flow.calls.iter().enumerate() {
        let mut targets = call_targets(host, file, flow, call);
        if targets.is_empty() {
            targets = callback_targets(host, file, flow, call);
        }
        for (target_path, target, via) in targets {
            calls.push(((target_path, Some(target)), call_index as u32, via));
        }
    }
    calls
}

/// Holds the [CallerIndex] between queries.
#[derive(Default)]
pub(crate) struct CallerIndexSlot(RwLock<Option<Arc<CallerIndex>>>);

impl CallerIndexSlot {
    fn get_or_build(&self, host: &dyn FlowHost) -> Arc<CallerIndex> {
        if let Some(index) = self.0.read().as_ref() {
            return Arc::clone(index);
        }
        let index = Arc::new(CallerIndex::build(host));
        *self.0.write() = Some(Arc::clone(&index));
        index
    }

    pub(crate) fn clear(&self) {
        *self.0.write() = None;
    }
}

impl std::fmt::Debug for CallerIndexSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallerIndexSlot")
            .field("built", &self.0.read().is_some())
            .finish()
    }
}

fn callers(
    host: &dyn FlowHost,
    start: Vec<FunctionKey>,
    options: CallGraphOptions,
) -> CallGraphResult {
    let index = match host.caller_index() {
        Some(slot) => slot.get_or_build(host),
        None => Arc::new(CallerIndex::build(host)),
    };
    let start: Vec<FunctionKey> = start
        .iter()
        .filter_map(|key| index.key_for(host, key))
        .collect();

    let mut entries = Entries::default();
    let mut truncated = false;
    let mut visited: HashSet<FunctionKey> = start.iter().cloned().collect();
    let mut reported: HashSet<(u32, u32)> = HashSet::new();
    let mut queue: VecDeque<(FunctionKey, u32)> = start.into_iter().map(|key| (key, 0)).collect();

    'walk: while let Some((key, depth)) = queue.pop_front() {
        if options.max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        for (file_id, call_index, via) in index.calls_of.get(&key).into_iter().flatten() {
            if !reported.insert((*file_id, *call_index)) {
                continue;
            }
            let file = &index.paths[*file_id as usize];
            let Some(flow) = index.flows.get(file) else {
                continue;
            };
            let call = &flow.calls[*call_index as usize];
            let caller = call.function.map(|f| &flow.functions[f as usize]);
            let entry = CallGraphEntry {
                range: caller.map_or(call.range, |f| f.range),
                name: Some(
                    caller
                        .map(|f| f.name.as_deref().unwrap_or("<anonymous>").to_string())
                        .unwrap_or_else(|| "<module>".to_string()),
                ),
                resolved: true,
                via: *via,
                depth: depth + 1,
                call_site: Some((file.clone(), call.range)),
            };
            if !entries.push(file, entry) {
                truncated = true;
                break 'walk;
            }
            let caller_key = (file.clone(), call.function);
            if options.transitive
                && call.function.is_some()
                && !options.stop_at.contains(*via)
                && visited.insert(caller_key.clone())
            {
                queue.push_back((caller_key, depth + 1));
            }
        }
    }
    entries.finish(
        |file| index.contents.get(file).cloned(),
        truncated,
        index.unparsed.clone(),
    )
}

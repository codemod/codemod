//! Queries over [FileFlow] facts: value flow, mutations and the call graph.
//!
//! Value flow and mutations stay inside one module. The call graph crosses
//! modules: an imported callee is resolved by the host, which is the same
//! import/re-export resolution that answers go-to-definition.

use crate::facts::{Accessor, CallFact, CalleeFact, FileFlow, Name};
use crate::once::BuildOnce;
use language_core::{
    ByteRange, CallEdgeKind, CallGraphDirection, CallGraphEntry, CallGraphFile, CallGraphOptions,
    CallGraphResult, EntryPoint, FileFlowSteps, FlowDirection, FlowEdgeKind, FlowNodeKind,
    FlowOptions, FlowResult, FlowStep, FunctionRef, Mutation, MutationKind, MutationOptions,
    MutationsResult, SemanticError, UnparsedFile,
};
use parking_lot::RwLock;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Most steps or call-graph entries one query returns.
const RESULT_LIMIT: usize = 50_000;

/// What the queries need from an analyzer.
pub trait FlowHost: Sync {
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
    /// When `name` is not declared in the module `specifier` but is a module
    /// of its own (Python's `from pkg import mod`), a specifier for it, seen
    /// from the same file.
    fn submodule(&self, _from: &Path, _specifier: &str, _name: &str) -> Option<String> {
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
    /// The workspace functions `object.property(…)` may call according to
    /// the type of `object`, for a call the values did not resolve. Empty
    /// when no type source is available.
    fn typed_method_targets(
        &self,
        _path: &Path,
        _flow: &FileFlow,
        _call: &CallFact,
    ) -> Vec<(PathBuf, u32)> {
        Vec::new()
    }
    /// Where the type checker says the member `object.property(…)` calls is
    /// declared, for a call that stays unresolved. None without types.
    fn typed_declaration(
        &self,
        _path: &Path,
        _flow: &FileFlow,
        _call: &CallFact,
    ) -> Option<FunctionRef> {
        None
    }
    /// Why the function at index `function` of `path` is called from outside
    /// the workspace's code: its package exports it, or a rule on the
    /// selector declares it. Empty without either.
    fn entry_points_of(&self, _path: &Path, _flow: &FileFlow, _function: u32) -> Vec<EntryPoint> {
        Vec::new()
    }
    /// `path` as answers show it: relative to the workspace root, with `/`.
    fn relative_path(&self, path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }
}

/// Which query [`applicable`] checks the node for.
#[derive(Clone, Copy)]
pub enum QueryKind {
    Flow,
    Mutations,
    Callers,
}

/// Whether a query applies to the node at `range`, or why not. The queries
/// themselves answer leniently (a range around a value answers for the
/// largest value inside it), so without this a statement would silently get
/// an answer about some expression in it.
pub fn applicable(
    host: &dyn FlowHost,
    path: &Path,
    range: ByteRange,
    kind: QueryKind,
) -> Result<(), SemanticError> {
    let path = host.canonical(path);
    let Some(flow) = host.flow(&path) else {
        return Ok(());
    };
    let not = |reason: &str| {
        Err(SemanticError::NotApplicable {
            reason: reason.to_string(),
        })
    };
    // A value at the range, or around it (a part of a value answers for it).
    let value = flow
        .locate(range)
        .filter(|&node| range.is_within(&flow.nodes[node as usize].range));
    match kind {
        QueryKind::Flow if value.is_none() => not(
            "there is no value here. Ask about an expression, a variable or parameter name, \
             or a call, not a statement or declaration",
        ),
        QueryKind::Mutations => {
            let Some(node) = value else {
                return not("there is no variable here. Ask about a variable's name or a use of it");
            };
            let is_variable = flow.nodes[node as usize].binding
                || flow.in_edges[node as usize]
                    .iter()
                    .any(|(_, edge)| *edge == FlowEdgeKind::Read);
            if is_variable {
                Ok(())
            } else {
                not("this is not a variable. Ask about a variable's name or a use of it (for `a.b`, ask about `a`)")
            }
        }
        // A call made by top-level code (the node a callers edge gives for
        // it, a decorator included) has no callers: the module runs it.
        QueryKind::Callers
            if functions_at(host, &path, range, true).is_empty()
                && flow.function_at(range).is_none()
                && !top_level_call(&flow, range) =>
        {
            not("there is no function here. Ask about a function, its name, a call to it, or code inside it")
        }
        _ => Ok(()),
    }
}

/// Whether `range` is a call top-level code makes: the call's own range,
/// or a node around it that starts with it (a Python `decorator` node runs
/// on to the end of its line).
fn top_level_call(flow: &FileFlow, range: ByteRange) -> bool {
    flow.calls.iter().any(|call| {
        call.function.is_none() && call.range.start == range.start && call.range.is_within(&range)
    })
}

pub fn data_flow(
    host: &dyn FlowHost,
    path: &Path,
    range: ByteRange,
    options: FlowOptions,
) -> FlowResult {
    if options.interprocedural {
        return data_flow_interprocedural(host, path, range, options);
    }
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
    // Each step's index, so a node reached later can point back at it.
    let mut seen_steps: HashMap<(u32, FlowEdgeKind, Option<u32>), u32> = HashMap::new();
    let mut queue = VecDeque::from([(start, 0u32, None::<u32>)]);

    while let Some((current, depth, reached_by)) = queue.pop_front() {
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
            let key = (other, edge, index);
            let this_step = if let Some(&existing) = seen_steps.get(&key) {
                Some(existing)
            } else {
                if steps.len() >= RESULT_LIMIT {
                    truncated = true;
                    break;
                }
                seen_steps.insert(key, steps.len() as u32);
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
                    from: reached_by,
                    from_file: None,
                });
                Some(steps.len() as u32 - 1)
            };
            // A returned or stored value leaves the function or goes into the
            // heap: reported, not followed.
            let stop = options.stop_at.contains(edge)
                || (!backward
                    && (matches!(edge, FlowEdgeKind::Return | FlowEdgeKind::Store)
                        || (call_edge && !options.through_calls)));
            if !stop && seen_nodes.insert(other) {
                queue.push_back((other, depth + 1, this_step));
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

pub fn mutations(
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
                    // f(): what the functions it calls return.
                    returned_origins(host, path, flow, current, seen, out);
                }
                continue;
            }
            FlowNodeKind::Function
            | FlowNodeKind::Class
            | FlowNodeKind::Object
            | FlowNodeKind::Array => {
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
                    } else if let Some(specifier) =
                        host.submodule(path, &import.module, &import.name)
                    {
                        out.push(Origin::Namespace(path.to_path_buf(), specifier));
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
        // `repo: OrderRepository`: whatever the declared type may be.
        if let Some(&declared) = flow.annotations.get(&current) {
            stack.push(declared);
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
        // `const [a, b] = v`, `v[1]`: the element of whatever v may be.
        for &(array, edge) in &flow.in_edges[current as usize] {
            if edge != FlowEdgeKind::Element {
                continue;
            }
            let position = match node.kind {
                FlowNodeKind::Member => node.name.as_deref().and_then(|n| n.parse::<u32>().ok()),
                _ => flow.element_index.get(&(array, current)).copied(),
            };
            let Some(position) = position else {
                continue;
            };
            let mut bases = Vec::new();
            origins(host, path, flow, array, seen, &mut bases);
            for base in &bases {
                if let Origin::Node(base_path, base_flow, base_node) = base {
                    let element = base_flow
                        .array_elements
                        .get(base_node)
                        .and_then(|elements| elements.get(position as usize).copied().flatten());
                    if let Some(element) = element {
                        origins(host, base_path, base_flow, element, seen, out);
                    }
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

/// Calls whose returns are being followed nest at most this deep.
const MAX_RETURN_DEPTH: usize = 8;

thread_local! {
    /// The calls returned_origins is resolving, on this thread.
    static RETURNS_IN_PROGRESS: std::cell::RefCell<Vec<(PathBuf, u32)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The values a call may return: the return values of the workspace
/// functions it calls (`const [s, setS] = createSignal()`).
fn returned_origins(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    call_node: u32,
    seen: &mut HashSet<(PathBuf, u32)>,
    out: &mut Vec<Origin>,
) {
    let Some(&call) = flow.call_by_node.get(&call_node) else {
        return;
    };
    let call = &flow.calls[call as usize];
    if call.is_new || call.is_jsx {
        return;
    }
    // f.bind(…) is f, with some arguments filled in.
    if let CalleeFact::Member { object, property } = &call.callee {
        if &**property == "bind" {
            origins(host, path, flow, *object, seen, out);
            return;
        }
    }
    // Resolving the call starts a new walk (call_targets), which can come
    // back here: f()() inside f, chains of factories. Each call is entered
    // once per walk, and no deeper than MAX_RETURN_DEPTH.
    let key = (path.to_path_buf(), call_node);
    let entered = RETURNS_IN_PROGRESS.with(|calls| {
        let mut calls = calls.borrow_mut();
        if calls.len() >= MAX_RETURN_DEPTH || calls.contains(&key) {
            false
        } else {
            calls.push(key.clone());
            true
        }
    });
    if !entered {
        return;
    }
    let targets = call_targets(host, path, flow, call);
    for (target_path, target, _) in targets {
        let Some(target_flow) = host.flow(&target_path) else {
            continue;
        };
        let function_node = target_flow.functions[target as usize].node;
        let mut returned: Vec<u32> = target_flow.in_edges[function_node as usize]
            .iter()
            .filter(|(_, edge)| *edge == FlowEdgeKind::Return)
            .map(|(value, _)| *value)
            .collect();
        // `def get_repo() -> Repo`: what the declared return type may be.
        if let Some(&declared) = target_flow.annotations.get(&function_node) {
            returned.push(declared);
        }
        for value in returned {
            origins(host, &target_path, &target_flow, value, seen, out);
        }
    }
    RETURNS_IN_PROGRESS.with(|calls| calls.borrow_mut().retain(|k| k != &key));
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
                .chain(flow.annotations.get(node).copied())
        })
        .collect()
}

/// The workspace functions a value may be (through aliases and imports).
pub fn functions_from(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    node: u32,
) -> Vec<(PathBuf, u32)> {
    let mut found = Vec::new();
    origins(host, path, flow, node, &mut HashSet::new(), &mut found);
    let mut functions = Vec::new();
    for origin in &found {
        if let Origin::Node(_, origin_flow, origin_node) = origin {
            if origin_flow.nodes[*origin_node as usize].kind == FlowNodeKind::Function {
                functions_of(host, origin, &mut functions);
            }
        }
    }
    functions
}

/// Classes deeper than this in an `extends` chain are not looked at.
const MAX_CLASS_DEPTH: u32 = 16;

/// The classes a class extends (`class A extends B`), through local aliases
/// and imports. Empty for `extends mixin(B)` and other expressions.
fn parent_classes(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    class_node: u32,
) -> Vec<(PathBuf, Arc<FileFlow>, u32)> {
    let Some(&parent) = flow.class_extends.get(&class_node) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    origins(host, path, flow, parent, &mut HashSet::new(), &mut found);
    found
        .into_iter()
        .filter_map(|origin| match origin {
            Origin::Node(path, flow, node)
                if flow.nodes[node as usize].kind == FlowNodeKind::Class =>
            {
                Some((path, flow, node))
            }
            _ => None,
        })
        .collect()
}

/// A class's constructor, or the nearest one up its `extends` chain.
fn constructor_of(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    class_node: u32,
    depth: u32,
    out: &mut Vec<(PathBuf, u32)>,
) {
    // `__init__` is Python's constructor.
    if let Some(function) = method_of(flow, class_node, "constructor")
        .or_else(|| method_of(flow, class_node, "__init__"))
    {
        out.push((path.to_path_buf(), function));
        return;
    }
    if depth >= MAX_CLASS_DEPTH {
        return;
    }
    for (parent_path, parent_flow, parent) in parent_classes(host, path, flow, class_node) {
        constructor_of(host, &parent_path, &parent_flow, parent, depth + 1, out);
    }
}

/// The functions an origin stands for when it is called.
fn functions_of(host: &dyn FlowHost, origin: &Origin, out: &mut Vec<(PathBuf, u32)>) {
    let Origin::Node(path, flow, node) = origin else {
        return;
    };
    match flow.nodes[*node as usize].kind {
        FlowNodeKind::Function => {
            if let Some(function) = flow.fn_by_node.get(node) {
                out.push((path.clone(), *function));
            }
        }
        FlowNodeKind::Class => constructor_of(host, path, flow, *node, 0, out),
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
            } else if let Some(submodule) = host.submodule(from, specifier, property) {
                out.push(Origin::Namespace(from.clone(), submodule));
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
                    let accessor = flow
                        .fn_by_node
                        .get(value)
                        .and_then(|f| flow.functions[*f as usize].accessor);
                    accessor_or_value(host, path, flow, *value, accessor, seen, out);
                }
            }
            FlowNodeKind::Class => class_member(path, flow, *node, property, host, seen, out),
            // new C().method()
            FlowNodeKind::Call => {
                // `new C()`, or `C()` where calling a class makes an
                // instance (Python): only class origins are used below.
                let constructed = flow
                    .call_by_node
                    .get(node)
                    .map(|call| &flow.calls[*call as usize])
                    .filter(|call| !call.is_jsx);
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

/// What reading a member gives: a plain member's value, a getter's
/// return values, nothing for a setter.
fn accessor_or_value(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    value: u32,
    accessor: Option<Accessor>,
    seen: &mut HashSet<(PathBuf, u32)>,
    out: &mut Vec<Origin>,
) {
    match accessor {
        None => origins(host, path, flow, value, seen, out),
        Some(Accessor::Set) => {}
        Some(Accessor::Get) => {
            let returned: Vec<u32> = flow.in_edges[value as usize]
                .iter()
                .filter(|(_, edge)| *edge == FlowEdgeKind::Return)
                .map(|(source, _)| *source)
                .collect();
            for source in returned {
                origins(host, path, flow, source, seen, out);
            }
        }
    }
}

/// A class's method, or the value of its field, else the nearest one up
/// its `extends` chain.
fn class_member(
    path: &Path,
    flow: &Arc<FileFlow>,
    class_node: u32,
    property: &str,
    host: &dyn FlowHost,
    seen: &mut HashSet<(PathBuf, u32)>,
    out: &mut Vec<Origin>,
) {
    class_member_at(path, flow, class_node, property, host, seen, out, 0);
}

#[allow(clippy::too_many_arguments)]
fn class_member_at(
    path: &Path,
    flow: &Arc<FileFlow>,
    class_node: u32,
    property: &str,
    host: &dyn FlowHost,
    seen: &mut HashSet<(PathBuf, u32)>,
    out: &mut Vec<Origin>,
    depth: u32,
) {
    if let Some(function) = method_of(flow, class_node, property) {
        let fact = &flow.functions[function as usize];
        accessor_or_value(host, path, flow, fact.node, fact.accessor, seen, out);
        return;
    }
    let fields = field_values(flow, class_node, property);
    if !fields.is_empty() {
        for value in fields {
            origins(host, path, flow, value, seen, out);
        }
        return;
    }
    if depth >= MAX_CLASS_DEPTH {
        return;
    }
    for (parent_path, parent_flow, parent) in parent_classes(host, path, flow, class_node) {
        class_member_at(
            &parent_path,
            &parent_flow,
            parent,
            property,
            host,
            seen,
            out,
            depth + 1,
        );
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
        functions_of(host, origin, out);
    }
}

/// The workspace functions a call may call, without asking the type
/// checker (the callers table, which asks lazily: see [CallerIndex]).
fn call_targets_untyped(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    call: &CallFact,
) -> Vec<(PathBuf, u32, CallEdgeKind)> {
    call_targets_with(host, path, flow, call, false)
}

/// The workspace functions a call may call.
pub fn call_targets(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    call: &CallFact,
) -> Vec<(PathBuf, u32, CallEdgeKind)> {
    call_targets_with(host, path, flow, call, true)
}

fn call_targets_with(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    call: &CallFact,
    typed: bool,
) -> Vec<(PathBuf, u32, CallEdgeKind)> {
    let mut functions = Vec::new();
    let mut seen = HashSet::new();
    let via = match &call.callee {
        CalleeFact::Member { object, property } => {
            let is_super = flow.super_nodes.contains(object);
            if is_super || flow.nodes[*object as usize].kind == FlowNodeKind::This {
                // this.m(): a method or function-valued field of the class or
                // its parents. super.m(): of its parents only.
                let mut found = Vec::new();
                if let Some(class) = flow.class_of(call.function) {
                    if is_super {
                        for (parent_path, parent_flow, parent) in
                            parent_classes(host, path, flow, class)
                        {
                            class_member(
                                &parent_path,
                                &parent_flow,
                                parent,
                                property,
                                host,
                                &mut seen,
                                &mut found,
                            );
                        }
                    } else {
                        class_member(path, flow, class, property, host, &mut seen, &mut found);
                    }
                }
                for origin in &found {
                    functions_of(host, origin, &mut functions);
                }
            } else {
                let mut found = Vec::new();
                origins(host, path, flow, *object, &mut seen, &mut found);
                for origin in &found {
                    member_functions(host, origin, property, &mut functions);
                }
                // f.call(this, …), f.apply(this, args): f.
                if functions.is_empty() && matches!(&**property, "call" | "apply") {
                    for origin in &found {
                        functions_of(host, origin, &mut functions);
                    }
                }
                if typed && functions.is_empty() && !call.is_jsx {
                    let mut typed = host.typed_method_targets(path, flow, call);
                    typed.dedup();
                    return typed
                        .into_iter()
                        .map(|(path, function)| (path, function, CallEdgeKind::Type))
                        .collect();
                }
            }
            if call.is_jsx {
                CallEdgeKind::Jsx
            } else {
                CallEdgeKind::Method
            }
        }
        // super(...): the parent class's constructor.
        CalleeFact::Node(callee) if flow.super_nodes.contains(callee) => {
            if let Some(class) = flow.class_of(call.function) {
                for (parent_path, parent_flow, parent) in parent_classes(host, path, flow, class) {
                    constructor_of(host, &parent_path, &parent_flow, parent, 0, &mut functions);
                }
            }
            CallEdgeKind::Constructor
        }
        CalleeFact::Node(callee) => {
            let mut found = Vec::new();
            origins(host, path, flow, *callee, &mut seen, &mut found);
            for origin in &found {
                functions_of(host, origin, &mut functions);
            }
            let class_call = found.iter().any(|origin| {
                matches!(origin, Origin::Node(_, f, n) if f.nodes[*n as usize].kind == FlowNodeKind::Class)
            });
            if call.is_new || class_call {
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
///
/// `resolved` are the call's own targets. When there are some, a function
/// argument is a callback only if one of them may call (or pass on) the
/// parameter it lands in: `register(fn)` storing `fn` counts, a parameter
/// that is never used does not.
pub fn callback_targets(
    host: &dyn FlowHost,
    path: &Path,
    flow: &Arc<FileFlow>,
    call: &CallFact,
    resolved: &[(PathBuf, u32, CallEdgeKind)],
) -> Vec<(PathBuf, u32, CallEdgeKind)> {
    let mut functions = Vec::new();
    let mut seen = HashSet::new();
    for (position, argument) in call.args.iter().enumerate() {
        if !resolved.is_empty()
            && !resolved.iter().any(|(target_path, target, _)| {
                parameter_may_be_called(host, target_path, *target, position as u32)
            })
        {
            continue;
        }
        let mut found = Vec::new();
        origins(host, path, flow, *argument, &mut seen, &mut found);
        for origin in &found {
            if let Origin::Node(_, origin_flow, node) = origin {
                if origin_flow.nodes[*node as usize].kind == FlowNodeKind::Function {
                    functions_of(host, origin, &mut functions);
                }
            }
        }
    }
    functions
        .into_iter()
        .map(|(path, function)| (path, function, CallEdgeKind::Callback))
        .collect()
}

/// Whether a function may call the value it gets as parameter `position`,
/// or hand it to code that might: it is called, passed to a call, stored,
/// captured, returned. False only when the parameter provably goes nowhere.
fn parameter_may_be_called(host: &dyn FlowHost, path: &Path, function: u32, position: u32) -> bool {
    let Some(flow) = host.flow(path) else {
        return true;
    };
    let parameter = flow.nodes.iter().position(|node| {
        node.binding
            && node.kind == FlowNodeKind::Parameter
            && node.index == Some(position)
            && node.function == Some(function)
    });
    // Destructured, rest, or `arguments`: no single binding to follow.
    let Some(parameter) = parameter else {
        return true;
    };
    let mut seen = HashSet::from([parameter as u32]);
    let mut stack = vec![parameter as u32];
    while let Some(current) = stack.pop() {
        let is_callee = flow.calls.iter().any(|call| match &call.callee {
            CalleeFact::Node(callee) => *callee == current,
            // cb.call(...), cb.apply(...)
            CalleeFact::Member { object, .. } => *object == current,
        });
        if is_callee {
            return true;
        }
        for &(next, edge) in &flow.out_edges[current as usize] {
            match edge {
                FlowEdgeKind::Read | FlowEdgeKind::Assign | FlowEdgeKind::Alias => {
                    if seen.insert(next) {
                        stack.push(next);
                    }
                }
                FlowEdgeKind::Property | FlowEdgeKind::Element | FlowEdgeKind::Derive => {}
                _ => return true,
            }
        }
    }
    false
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
                functions_of(host, origin, &mut functions);
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
/// The workspace functions a declaration the type checker pointed at
/// stands for: the outermost functions written inside it (a method, an
/// arrow-function field or property), else the functions its value may be
/// (`{ m }`, `m: f`, `m = f`).
pub fn declaration_functions(
    host: &dyn FlowHost,
    path: &Path,
    range: ByteRange,
) -> Vec<(PathBuf, u32)> {
    let Some(flow) = host.flow(path) else {
        return Vec::new();
    };
    let written: Vec<(PathBuf, u32)> = outermost_functions(&flow, path, range)
        .into_iter()
        .filter_map(|(path, function)| Some((path, function?)))
        .collect();
    if !written.is_empty() {
        return written;
    }
    // The value written to the member.
    let mut values: Vec<u32> = flow
        .object_members
        .values()
        .chain(flow.class_fields.values())
        .flatten()
        .map(|(_, node)| *node)
        .filter(|node| flow.nodes[*node as usize].range.is_within(&range))
        .collect();
    if values.is_empty() {
        values.extend(
            flow.nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.range.is_within(&range))
                .max_by_key(|(_, n)| n.range.len())
                .map(|(index, _)| index as u32),
        );
    }
    let mut functions = Vec::new();
    let mut seen = HashSet::new();
    for value in values {
        let mut found = Vec::new();
        origins(host, path, &flow, value, &mut seen, &mut found);
        for origin in &found {
            functions_of(host, origin, &mut functions);
        }
    }
    functions.sort();
    functions.dedup();
    functions
}

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

pub fn call_graph(
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
            let start = target_functions(host, &path, range);
            if start.is_empty() {
                return CallGraphResult::default();
            }
            callers(host, start, options)
        }
    }
}

/// The functions a callers question about `range` is about: the function
/// it names or is, else the one it is in.
fn target_functions(host: &dyn FlowHost, path: &Path, range: ByteRange) -> Vec<FunctionKey> {
    let start = functions_at(host, path, range, true);
    if !start.is_empty() {
        return start;
    }
    host.flow(path)
        .and_then(|flow| flow.function_at(range))
        .map(|function| vec![(path.to_path_buf(), Some(function))])
        .unwrap_or_default()
}

/// Entry points of `key`.
fn entry_points_of_key(host: &dyn FlowHost, key: &FunctionKey) -> Vec<EntryPoint> {
    let Some(function) = key.1 else {
        return Vec::new();
    };
    let Some(flow) = host.flow(&key.0) else {
        return Vec::new();
    };
    host.entry_points_of(&key.0, &flow, function)
}

/// Why the function at `range` is called from outside the workspace's
/// code. `callers()` never includes these: an entry point is not a call site.
pub fn entry_points(host: &dyn FlowHost, path: &Path, range: ByteRange) -> Vec<EntryPoint> {
    let path = host.canonical(path);
    let mut found: Vec<EntryPoint> = target_functions(host, &path, range)
        .iter()
        .flat_map(|key| entry_points_of_key(host, key))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Every function in the workspace that is called from outside its code,
/// with why: `(file, function range, entry points)`, in file and range order.
pub fn all_entry_points(host: &dyn FlowHost) -> Vec<(PathBuf, ByteRange, Vec<EntryPoint>)> {
    let mut found = Vec::new();
    for path in host.workspace_files() {
        let Some(flow) = host.flow(&path) else {
            continue;
        };
        for (index, fact) in flow.functions.iter().enumerate() {
            let mut entries = host.entry_points_of(&path, &flow, index as u32);
            if entries.is_empty() {
                continue;
            }
            entries.sort();
            entries.dedup();
            found.push((path.clone(), fact.range, entries));
        }
    }
    found.sort_by(|a, b| (&a.0, a.1.start, a.1.end).cmp(&(&b.0, b.1.start, b.1.end)));
    found
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
    // One entry per call site and callee (as callers() answers per call).
    let mut reported: HashSet<(FunctionKey, PathBuf, u32)> = HashSet::new();
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
                    callee: None,
                    declaration: host.typed_declaration(&path, &flow, call),
                };
                if !entries.push(&path, unresolved) {
                    truncated = true;
                    break 'walk;
                }
            }
            let callbacks = callback_targets(host, &path, &flow, call, &targets);
            targets.extend(callbacks);
            for (target_path, target, via) in targets {
                let key = (target_path.clone(), Some(target));
                // Direct callees: one entry per call site. Deeper: per function.
                let site = if depth == 0 {
                    (path.clone(), call.range.start)
                } else {
                    (PathBuf::new(), 0)
                };
                if !reported.insert((key.clone(), site.0, site.1)) {
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
                    callee: None,
                    declaration: None,
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

/// A call in the callers table: calling file id, call index, how.
type CallSiteRef = (u32, u32, CallEdgeKind);

/// Every call in the workspace, by the function it may call, with the facts
/// and content it was built from. Kept between queries (see
/// [CallerIndexSlot]), so callers answers are a snapshot of the workspace as
/// it was when the first callers query ran.
pub struct CallerIndex {
    /// What callers answers need of each file, as it was: its functions and
    /// calls. The full facts are not kept, so they can leave the cache.
    sites: HashMap<PathBuf, FileSites>,
    contents: HashMap<PathBuf, Arc<str>>,
    /// By called function (file id, function index): calling file id, call
    /// index, how.
    calls_of: FxHashMap<(u32, u32), Vec<CallSiteRef>>,
    /// Files by id: the workspace files in path order, then files only
    /// called into.
    paths: Vec<PathBuf>,
    file_ids: HashMap<PathBuf, u32>,
    /// Member calls (`x.m()`) the values left unresolved, by property name:
    /// (file id, call index). Types resolve them lazily, for the names a
    /// callers query asks about, instead of asking the checker about every
    /// call while the table is built.
    unresolved_members: HashMap<Name, Vec<(u32, u32)>>,
    /// Edges found through types so far, and the names already asked about.
    typed: RwLock<TypedCalls>,
    unparsed: Vec<UnparsedFile>,
}

/// Calls found through types, by called function.
#[derive(Default)]
struct TypedCalls {
    names: HashSet<Name>,
    calls_of: HashMap<FunctionKey, Vec<(u32, u32, CallEdgeKind)>>,
}

impl CallerIndex {
    fn build(host: &dyn FlowHost) -> Self {
        let mut index = CallerIndex {
            sites: HashMap::new(),
            contents: HashMap::new(),
            calls_of: FxHashMap::default(),
            paths: Vec::new(),
            file_ids: HashMap::new(),
            unresolved_members: HashMap::new(),
            typed: RwLock::new(TypedCalls::default()),
            unparsed: Vec::new(),
        };
        let mut files = host.workspace_files();
        files.sort();
        // Each file's calls are resolved on their own, in parallel; the
        // table is then put together in file order. Only the compact sites
        // are kept, so a file's full facts can leave the cache once done.
        let contributions: Vec<_> = files
            .into_par_iter()
            .filter_map(|file| {
                let (Some(flow), Some(content)) = (host.flow(&file), host.content(&file)) else {
                    return None;
                };
                let (calls, unresolved) = file_calls(host, &file, &flow);
                let sites = FileSites::of(&flow);
                Some((file, sites, content, calls, unresolved))
            })
            .collect();
        for (file, ..) in &contributions {
            index.file_id(file);
        }
        for (file, sites, content, calls, unresolved) in contributions {
            let file_id = index.file_ids[&file];
            for (name, call_index) in unresolved {
                index
                    .unresolved_members
                    .entry(name)
                    .or_default()
                    .push((file_id, call_index));
            }
            for ((target_path, target), call_index, via) in calls {
                let Some(target) = target else {
                    continue;
                };
                let target_file = index.file_id(&target_path);
                index
                    .calls_of
                    .entry((target_file, target))
                    .or_default()
                    .push((file_id, call_index, via));
            }
            index.sites.insert(file.clone(), sites);
            index.contents.insert(file, content);
        }
        index.unparsed = host.parse_failures();
        index
    }

    /// The id of a file, given one if it has none yet.
    fn file_id(&mut self, path: &Path) -> u32 {
        if let Some(id) = self.file_ids.get(path) {
            return *id;
        }
        let id = self.paths.len() as u32;
        self.paths.push(path.to_path_buf());
        self.file_ids.insert(path.to_path_buf(), id);
        id
    }

    /// The calls of a function: the table's, plus those found through
    /// types. The first time a function name is asked about, the unresolved
    /// member calls with that property name are resolved by type (when the
    /// host has types), in files whose content has not changed since the
    /// table was built.
    fn calls_to(&self, host: &dyn FlowHost, key: &FunctionKey) -> Vec<(u32, u32, CallEdgeKind)> {
        let (path, Some(function)) = key else {
            return Vec::new();
        };
        let mut out: Vec<_> = self
            .file_ids
            .get(path)
            .and_then(|file| self.calls_of.get(&(*file, *function)))
            .cloned()
            .unwrap_or_default();
        let name = self
            .sites
            .get(path)
            .and_then(|sites| sites.functions.get(*function as usize)?.1.clone());
        if let Some(name) = name {
            if !self.typed.read().names.contains(&name) {
                self.resolve_typed(host, &name);
            }
            if let Some(typed) = self.typed.read().calls_of.get(key) {
                out.extend(typed.iter().copied());
            }
        }
        out
    }

    fn resolve_typed(&self, host: &dyn FlowHost, name: &Name) {
        let mut found: Vec<(FunctionKey, (u32, u32, CallEdgeKind))> = Vec::new();
        for (file_id, call_index) in self.unresolved_members.get(name).into_iter().flatten() {
            let path = &self.paths[*file_id as usize];
            let (Some(flow), Some(content)) = (host.flow(path), host.content(path)) else {
                continue;
            };
            let unchanged = self
                .contents
                .get(path)
                .is_some_and(|then| Arc::ptr_eq(then, &content) || **then == *content);
            let Some(call) = flow.calls.get(*call_index as usize).filter(|_| unchanged) else {
                continue;
            };
            for (target_path, target) in host.typed_method_targets(path, &flow, call) {
                found.push((
                    (target_path, Some(target)),
                    (*file_id, *call_index, CallEdgeKind::Type),
                ));
            }
        }
        let mut typed = self.typed.write();
        typed.names.insert(name.clone());
        for (target, call) in found {
            let calls = typed.calls_of.entry(target).or_default();
            if !calls.contains(&call) {
                calls.push(call);
            }
        }
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
        let Some(snapshot) = self.sites.get(path) else {
            return Some(key.clone());
        };
        let current = host.flow(path)?;
        // Same content, same facts: the function has the same index.
        let unchanged = match (self.contents.get(path), host.content(path)) {
            (Some(then), Some(now)) => Arc::ptr_eq(then, &now) || then == &now,
            _ => false,
        };
        if unchanged {
            return Some(key.clone());
        }
        let fact = current.functions.get(*function as usize)?;
        snapshot
            .functions
            .iter()
            .position(|(range, name)| *range == fact.range && *name == fact.name)
            .map(|index| (path.clone(), Some(index as u32)))
    }
}

/// A file's functions (range, name) and calls (range, calling function),
/// as the callers table saw them.
struct FileSites {
    functions: Box<[(ByteRange, Option<Name>)]>,
    calls: Box<[(ByteRange, Option<u32>)]>,
}

impl FileSites {
    fn of(flow: &FileFlow) -> Self {
        FileSites {
            functions: flow
                .functions
                .iter()
                .map(|f| (f.range, f.name.clone()))
                .collect(),
            calls: flow.calls.iter().map(|c| (c.range, c.function)).collect(),
        }
    }
}

/// One file's contribution to the callers table: each call's targets, as
/// (target function, call index, how), and the member calls left for types
/// (property name, call index). Depends only on the file's facts and what
/// its imports resolve to, so a changed file's part can be rebuilt alone.
#[allow(clippy::type_complexity)]
fn file_calls(
    host: &dyn FlowHost,
    file: &Path,
    flow: &Arc<FileFlow>,
) -> (Vec<(FunctionKey, u32, CallEdgeKind)>, Vec<(Name, u32)>) {
    let mut calls = Vec::new();
    let mut unresolved = Vec::new();
    for (call_index, call) in flow.calls.iter().enumerate() {
        let mut targets = call_targets_untyped(host, file, flow, call);
        if targets.is_empty() && !call.is_jsx {
            if let CalleeFact::Member { property, .. } = &call.callee {
                unresolved.push((property.clone(), call_index as u32));
            }
        }
        let callbacks = callback_targets(host, file, flow, call, &targets);
        targets.extend(callbacks);
        for (target_path, target, via) in targets {
            calls.push(((target_path, Some(target)), call_index as u32, via));
        }
    }
    (calls, unresolved)
}

/// Holds the [CallerIndex] between queries. Built once: threads that ask
/// while it is being built wait for that build (see [`BuildOnce`]).
#[derive(Default)]
pub struct CallerIndexSlot(BuildOnce<CallerIndex>);

impl CallerIndexSlot {
    fn get_or_build(&self, host: &dyn FlowHost) -> Arc<CallerIndex> {
        self.0.get_or_build(|| CallerIndex::build(host))
    }

    pub fn clear(&self) {
        self.0.clear();
    }
}

impl std::fmt::Debug for CallerIndexSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallerIndexSlot")
            .field("built", &self.0.get().is_some())
            .finish()
    }
}

/// The function `key` stands for, as the callers walk reports it.
fn callee_ref(index: &CallerIndex, key: &FunctionKey) -> Option<FunctionRef> {
    let (range, name) = index.sites.get(&key.0)?.functions.get(key.1? as usize)?;
    Some(FunctionRef {
        file_path: key.0.clone(),
        range: *range,
        name: name.as_deref().map(str::to_string),
    })
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
        for (file_id, call_index, via) in index.calls_to(host, &key).iter() {
            if !reported.insert((*file_id, *call_index)) {
                continue;
            }
            let file = &index.paths[*file_id as usize];
            let Some(sites) = index.sites.get(file) else {
                continue;
            };
            let (call_range, call_function) = sites.calls[*call_index as usize];
            let caller = call_function.map(|f| &sites.functions[f as usize]);
            let entry = CallGraphEntry {
                range: caller.map_or(call_range, |(range, _)| *range),
                name: Some(
                    caller
                        .map(|(_, name)| name.as_deref().unwrap_or("<anonymous>").to_string())
                        .unwrap_or_else(|| "<module>".to_string()),
                ),
                resolved: true,
                via: *via,
                depth: depth + 1,
                call_site: Some((file.clone(), call_range)),
                callee: callee_ref(&index, &key),
                declaration: None,
            };
            if !entries.push(file, entry) {
                truncated = true;
                break 'walk;
            }
            let caller_key = (file.clone(), call_function);
            if options.transitive
                && call_function.is_some()
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

/// One file of an interprocedural walk: its facts and the steps found in it.
struct WalkFile {
    path: PathBuf,
    flow: Arc<FileFlow>,
    content: Arc<str>,
    steps: Vec<FlowStep>,
    /// (node, edge, index) -> step, so a step is reported once.
    step_at: HashMap<(u32, FlowEdgeKind, Option<u32>), u32>,
}

/// Calls an interprocedural walk remembers: entering a callee deeper than
/// this treats the call as unknown (its result derived from its arguments).
const MAX_CALL_STACK: usize = 4;
/// Times a walk may leave a function it did not enter through a call
/// (a parameter to every caller, a return to every call site).
const MAX_ASCENTS: u8 = 4;
/// (node, call context) pairs one interprocedural walk may visit. The
/// contexts multiply the nodes, so without this a walk over a large
/// workspace can run for a very long time; past it the answer is cut and
/// `truncated` is set.
pub(crate) const MAX_WALK_STATES: usize = 20_000;
/// Call sites one such step fans out to, when the query does not say.
pub(crate) const DEFAULT_MAX_CALLERS: u32 = 16;

/// The calls an interprocedural walk came in through (innermost last), and
/// how many times it has left a function it did not enter.
#[derive(Clone, Default, PartialEq, Eq, Hash)]
struct CallContext {
    stack: Vec<(usize, u32)>,
    ascents: u8,
}

impl CallContext {
    fn enter(&self, site: (usize, u32)) -> Option<CallContext> {
        (self.stack.len() < MAX_CALL_STACK).then(|| {
            let mut stack = self.stack.clone();
            stack.push(site);
            CallContext {
                stack,
                ascents: self.ascents,
            }
        })
    }
}

/// Where an interprocedural walk is: a file (index), a node, and the calls
/// it came in through. Leaving a function goes back to the call it was
/// entered through, if any.
type WalkAt = (usize, u32, CallContext);
/// A step in a walk: file index, step index.
type StepRef = (usize, u32);
/// A queued node: where, depth, and the step it was reached by.
type Queued = (WalkAt, u32, Option<StepRef>);
/// A step to report: file, node, via, index, boundary, keep walking from
/// it, and the call its function was entered through.
type NextStep = (
    usize,
    u32,
    FlowEdgeKind,
    Option<u32>,
    bool,
    bool,
    CallContext,
);

struct Walk<'h> {
    host: &'h dyn FlowHost,
    backward: bool,
    files: Vec<WalkFile>,
    file_ids: HashMap<PathBuf, usize>,
    callers: Option<Arc<CallerIndex>>,
    truncated: bool,
    step_count: usize,
}

impl<'h> Walk<'h> {
    fn open(&mut self, path: &Path) -> Option<usize> {
        if let Some(&id) = self.file_ids.get(path) {
            return Some(id);
        }
        let flow = self.host.flow(path)?;
        let content = self.host.content(path)?;
        let id = self.files.len();
        self.files.push(WalkFile {
            path: path.to_path_buf(),
            flow,
            content,
            steps: Vec::new(),
            step_at: HashMap::new(),
        });
        self.file_ids.insert(path.to_path_buf(), id);
        Some(id)
    }

    /// The workspace functions a call resolves to (not `new`, not JSX).
    fn targets(&mut self, file: usize, call_node: u32) -> Vec<(usize, u32)> {
        let flow = Arc::clone(&self.files[file].flow);
        let Some(&call) = flow.call_by_node.get(&call_node) else {
            return Vec::new();
        };
        let call = &flow.calls[call as usize];
        if call.is_jsx {
            return Vec::new();
        }
        let path = self.files[file].path.clone();
        call_targets(self.host, &path, &flow, call)
            .into_iter()
            .filter_map(|(target_path, function, _)| Some((self.open(&target_path)?, function)))
            .collect()
    }

    /// Call sites of a function, from the workspace callers table: (file,
    /// call node), for calls that pass arguments positionally.
    fn call_sites(&mut self, file: usize, function: u32) -> Vec<(usize, u32)> {
        let index = match &self.callers {
            Some(index) => Arc::clone(index),
            None => {
                let index = match self.host.caller_index() {
                    Some(slot) => slot.get_or_build(self.host),
                    None => Arc::new(CallerIndex::build(self.host)),
                };
                self.callers = Some(Arc::clone(&index));
                index
            }
        };
        let key = (self.files[file].path.clone(), Some(function));
        let Some(key) = index.key_for(self.host, &key) else {
            return Vec::new();
        };
        let mut sites = Vec::new();
        for (file_id, call_index, via) in index.calls_to(self.host, &key).iter() {
            if matches!(via, CallEdgeKind::Callback) {
                continue;
            }
            let path = index.paths[*file_id as usize].clone();
            let Some(site_file) = self.open(&path) else {
                continue;
            };
            let flow = &self.files[site_file].flow;
            // The table is a snapshot: skip a call the file no longer has.
            let Some(call) = flow.calls.get(*call_index as usize) else {
                continue;
            };
            let expected = index
                .sites
                .get(&path)
                .and_then(|sites| sites.calls.get(*call_index as usize))
                .map(|(range, _)| *range);
            if expected != Some(call.range) {
                continue;
            }
            if let Some(node) = flow.exact(call.range) {
                sites.push((site_file, node));
            }
        }
        sites
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        file: usize,
        node: u32,
        via: FlowEdgeKind,
        index: Option<u32>,
        depth: u32,
        from: Option<(usize, u32)>,
        boundary: bool,
    ) -> Option<(usize, u32)> {
        if let Some(&existing) = self.files[file].step_at.get(&(node, via, index)) {
            return Some((file, existing));
        }
        if self.step_count >= RESULT_LIMIT {
            self.truncated = true;
            return None;
        }
        self.step_count += 1;
        let target = &mut self.files[file];
        let n = &target.flow.nodes[node as usize];
        let step = FlowStep {
            range: n.range,
            kind: n.kind,
            via,
            name: n.name.as_deref().map(str::to_string),
            index,
            depth,
            boundary,
            from: from.map(|(_, step)| step),
            from_file: from
                .and_then(|(from_file, _)| (from_file != file).then_some(from_file as u32)),
        };
        target.steps.push(step);
        let at = target.steps.len() as u32 - 1;
        target.step_at.insert((node, via, index), at);
        Some((file, at))
    }

    /// Parameter bindings of a function at a position (several for a
    /// destructured parameter).
    fn parameters(&self, file: usize, function: u32, position: u32) -> Vec<u32> {
        let flow = &self.files[file].flow;
        (0..flow.nodes.len() as u32)
            .filter(|&n| {
                let node = &flow.nodes[n as usize];
                node.binding
                    && node.kind == FlowNodeKind::Parameter
                    && node.index == Some(position)
                    && node.function == Some(function)
            })
            .collect()
    }
}

/// [`data_flow`] with `interprocedural: true`.
fn data_flow_interprocedural(
    host: &dyn FlowHost,
    path: &Path,
    range: ByteRange,
    options: FlowOptions,
) -> FlowResult {
    let mut walk = Walk {
        host,
        backward: options.direction == FlowDirection::Backward,
        files: Vec::new(),
        file_ids: HashMap::new(),
        callers: None,
        truncated: false,
        step_count: 0,
    };
    let max_callers = options.max_callers.unwrap_or(DEFAULT_MAX_CALLERS) as usize;
    // A cap cut the answer short (it goes on with what it has).
    let mut capped = false;
    let Some(file) = walk.open(&host.canonical(path)) else {
        return FlowResult::default();
    };
    let Some(start) = walk.files[file].flow.locate(range) else {
        return FlowResult::default();
    };
    let backward = walk.backward;
    let mut seen: HashSet<WalkAt> = HashSet::from([(file, start, CallContext::default())]);
    let mut queue: VecDeque<Queued> =
        VecDeque::from([((file, start, CallContext::default()), 0, None)]);

    // Leaving a function not entered through a call: where to, within the caps.
    // Returns the call sites with their context, and whether a cap cut them.
    let ascend = |walk: &mut Walk, file: usize, function: u32, context: &CallContext| {
        if context.ascents >= MAX_ASCENTS {
            return (Vec::new(), true);
        }
        let mut sites = walk.call_sites(file, function);
        let cut = sites.len() > max_callers;
        sites.truncate(max_callers);
        let up = CallContext {
            stack: Vec::new(),
            ascents: context.ascents + 1,
        };
        let sites: Vec<_> = sites.into_iter().map(|site| (site, up.clone())).collect();
        (sites, cut)
    };

    while let Some(((file, current, context), depth, reached_by)) = queue.pop_front() {
        if walk.truncated || options.max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        let flow = Arc::clone(&walk.files[file].flow);
        let node = flow.nodes[current as usize].clone();
        // (file, node, via, index, boundary, follow, entered call)
        let mut next: Vec<NextStep> = Vec::new();

        // A call into the workspace: its result is what the callee returns
        // (while the call stack has room; past that, the call is unknown).
        let inner = context.enter((file, current));
        let resolved_call = node.kind == FlowNodeKind::Call && inner.is_some() && {
            let targets = walk.targets(file, current);
            if backward {
                for &(target_file, function) in &targets {
                    let target_flow = Arc::clone(&walk.files[target_file].flow);
                    let function_node = target_flow.functions[function as usize].node;
                    for &(value, edge) in &target_flow.in_edges[function_node as usize] {
                        if edge == FlowEdgeKind::Return {
                            next.push((
                                target_file,
                                value,
                                FlowEdgeKind::Return,
                                None,
                                is_boundary(target_flow.nodes[value as usize].kind),
                                true,
                                inner.clone().unwrap_or_default(),
                            ));
                        }
                    }
                }
            }
            !targets.is_empty()
        };

        // A parameter: the arguments at its callers (only the call the walk
        // came in through, when it did).
        if backward && node.binding && node.kind == FlowNodeKind::Parameter {
            if let (Some(function), Some(position)) = (node.function, node.index) {
                let sites = match context.stack.split_last() {
                    Some((site, rest)) => vec![(
                        *site,
                        CallContext {
                            stack: rest.to_vec(),
                            ascents: context.ascents,
                        },
                    )],
                    None => {
                        let (sites, cut) = ascend(&mut walk, file, function, &context);
                        capped |= cut;
                        sites
                    }
                };
                for ((site_file, call_node), site_context) in sites {
                    let site_flow = Arc::clone(&walk.files[site_file].flow);
                    let call = &site_flow.calls[site_flow.call_by_node[&call_node] as usize];
                    if let Some(&argument) = call.args.get(position as usize) {
                        let n = &site_flow.nodes[argument as usize];
                        let boundary = is_boundary(n.kind);
                        next.push((
                            site_file,
                            argument,
                            FlowEdgeKind::Assign,
                            Some(position),
                            boundary,
                            true,
                            site_context,
                        ));
                    }
                }
            }
        }

        let edges: Vec<(u32, FlowEdgeKind)> = if backward {
            flow.in_edges[current as usize].to_vec()
        } else {
            flow.out_edges[current as usize].to_vec()
        };
        for (other, edge) in edges {
            let call_edge = matches!(edge, FlowEdgeKind::Argument | FlowEdgeKind::Receiver);
            if backward && (edge == FlowEdgeKind::Return || (call_edge && !options.through_calls)) {
                continue;
            }
            // The callee is known: its returns stand for "derived from the
            // arguments".
            if backward && call_edge && resolved_call {
                continue;
            }
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
            if !backward && edge == FlowEdgeKind::Argument {
                let inner = context.enter((file, other));
                let targets = if inner.is_some() {
                    walk.targets(file, other)
                } else {
                    Vec::new()
                };
                if let (false, Some(inner)) = (targets.is_empty(), inner) {
                    // Into the callee's parameter, remembering the call.
                    let position = flow.call_by_node.get(&other).and_then(|c| {
                        flow.calls[*c as usize]
                            .args
                            .iter()
                            .position(|a| *a == current)
                    });
                    if let Some(position) = position {
                        for (target_file, function) in targets {
                            for parameter in walk.parameters(target_file, function, position as u32)
                            {
                                next.push((
                                    target_file,
                                    parameter,
                                    FlowEdgeKind::Assign,
                                    Some(position as u32),
                                    false,
                                    true,
                                    inner.clone(),
                                ));
                            }
                        }
                    }
                    continue;
                }
            }
            if !backward && edge == FlowEdgeKind::Return {
                // Out of the function: to the call it was entered through, or
                // to every call site.
                let Some(&function) = flow.fn_by_node.get(&other) else {
                    continue;
                };
                let sites = match context.stack.split_last() {
                    Some((site, rest)) => vec![(
                        *site,
                        CallContext {
                            stack: rest.to_vec(),
                            ascents: context.ascents,
                        },
                    )],
                    None => {
                        let (sites, cut) = ascend(&mut walk, file, function, &context);
                        capped |= cut;
                        sites
                    }
                };
                for ((site_file, call_node), site_context) in sites {
                    next.push((
                        site_file,
                        call_node,
                        FlowEdgeKind::Return,
                        None,
                        false,
                        true,
                        site_context,
                    ));
                }
                continue;
            }
            let n = &flow.nodes[other as usize];
            let index = if edge == FlowEdgeKind::Argument {
                let (call, argument) = if backward {
                    (current, other)
                } else {
                    (other, current)
                };
                flow.call_by_node.get(&call).and_then(|c| {
                    flow.calls[*c as usize]
                        .args
                        .iter()
                        .position(|a| *a == argument)
                        .map(|p| p as u32)
                })
            } else {
                n.index
            };
            let follow = !(!backward
                && (edge == FlowEdgeKind::Store || (call_edge && !options.through_calls)));
            next.push((
                file,
                other,
                edge,
                index,
                backward && is_boundary(n.kind),
                follow,
                context.clone(),
            ));
        }

        for (to_file, to, via, index, boundary, follow, to_context) in next {
            // Parameters and calls are boundaries only when the walk cannot
            // go on through them.
            let boundary = boundary && {
                let kind = walk.files[to_file].flow.nodes[to as usize].kind;
                match kind {
                    FlowNodeKind::Call => walk.targets(to_file, to).is_empty(),
                    FlowNodeKind::Parameter => {
                        let n = walk.files[to_file].flow.nodes[to as usize].clone();
                        match (n.function, n.binding) {
                            (Some(function), true) => {
                                to_context.stack.is_empty()
                                    && (to_context.ascents >= MAX_ASCENTS
                                        || walk.call_sites(to_file, function).is_empty())
                            }
                            _ => true,
                        }
                    }
                    _ => true,
                }
            };
            let Some(step) = walk.push(to_file, to, via, index, depth + 1, reached_by, boundary)
            else {
                break;
            };
            let stop = !follow || options.stop_at.contains(via);
            if stop {
                continue;
            }
            if seen.len() >= MAX_WALK_STATES {
                capped = true;
                continue;
            }
            if seen.insert((to_file, to, to_context.clone())) {
                queue.push_back(((to_file, to, to_context), depth + 1, Some(step)));
            }
        }
    }

    FlowResult {
        files: walk
            .files
            .into_iter()
            .map(|f| FileFlowSteps {
                file_path: f.path,
                content: f.content.to_string(),
                steps: f.steps,
            })
            .collect(),
        truncated: walk.truncated || capped,
    }
}

fn is_boundary(kind: FlowNodeKind) -> bool {
    matches!(
        kind,
        FlowNodeKind::Parameter
            | FlowNodeKind::Import
            | FlowNodeKind::Global
            | FlowNodeKind::This
            | FlowNodeKind::Call
    )
}

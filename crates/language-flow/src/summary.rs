//! Per-function data-flow summaries, and the one-hop lookup answers that
//! use them to follow values across calls.
//!
//! A summary says, for each parameter of a function, whether the value it
//! gets can come back out as the function's result, and whether the
//! function stores it somewhere that outlives the call. It is computed once
//! per function from the function's own facts (and its callees' summaries)
//! and reused at every call site, so a walk never has to go into a callee to
//! learn what its call returns.
//!
//! That is what keeps cross-call answers precise: a value passed by one
//! caller comes back to that caller's call, through the call's own
//! `argument` step, and never to another caller of the same function. The
//! steps into a callee (`parameter` forward, `return` backward) are there
//! for anyone who wants the path inside it; a walk that has taken one should
//! not then take a step out of the function to its callers
//! ([`Crossing::Out`]).

use crate::facts::{CallFact, FileFlow, Name, Variadic};
use crate::query::{call_sites_of, call_targets, is_boundary, FlowHost};
use language_core::{
    ByteRange, CallEdgeKind, Crossing, FileFlowSteps, FlowDirection, FlowEdgeKind, FlowNodeKind,
    FlowOptions, FlowResult, FlowStep, MutationKind,
};
use parking_lot::RwLock;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Summaries being computed at once (a call chain): past this, a callee is
/// treated as unknown, like a recursive one.
const MAX_NESTED: usize = 32;

/// What a function does with the value one parameter gets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParamFlow {
    /// The value, or something computed from it, can be the result.
    pub returns: bool,
    /// The value is stored where it outlives the call: a property of
    /// `this` or of something the function did not create, or a variable
    /// outside the function.
    pub stored: bool,
}

/// What a function does with its parameters' values.
#[derive(Debug, Default)]
pub struct Summary {
    /// By parameter position.
    pub params: Vec<ParamFlow>,
    /// The function reads `arguments`: any argument may go anywhere.
    pub opaque: bool,
}

impl Summary {
    /// What happens to the value at parameter `position`.
    pub fn param(&self, position: u32) -> ParamFlow {
        if self.opaque {
            return ParamFlow {
                returns: true,
                stored: true,
            };
        }
        self.params
            .get(position as usize)
            .copied()
            .unwrap_or_default()
    }
}

/// What summaries need of a file, worked out once per file.
struct FileInfo {
    /// Function index -> its parameter bindings.
    params: Vec<Vec<u32>>,
    /// Functions that read `arguments`.
    uses_arguments: FxHashSet<u32>,
}

impl FileInfo {
    fn of(flow: &FileFlow) -> Self {
        let mut params = vec![Vec::new(); flow.functions.len()];
        let mut uses_arguments = FxHashSet::default();
        for (id, node) in flow.nodes.iter().enumerate() {
            match (node.kind, node.function) {
                (FlowNodeKind::Parameter, Some(function)) if node.binding => {
                    params[function as usize].push(id as u32);
                }
                (FlowNodeKind::Global, Some(function))
                    if node.name.as_deref() == Some("arguments") =>
                {
                    uses_arguments.insert(function);
                }
                _ => {}
            }
        }
        FileInfo {
            params,
            uses_arguments,
        }
    }
}

type Key = (PathBuf, u32);
/// A cached value and the file content it was computed from.
type Cached<T> = (Arc<str>, Arc<T>);

/// Summaries and file info kept between queries, next to the callers table
/// (cleared with it, when the workspace changes). Each entry remembers the
/// content it was computed from and is ignored once the file changes.
#[derive(Default)]
pub struct SummaryCache {
    summaries: RwLock<FxHashMap<Key, Cached<Summary>>>,
    files: RwLock<FxHashMap<PathBuf, Cached<FileInfo>>>,
}

impl SummaryCache {
    pub fn clear(&self) {
        self.summaries.write().clear();
        self.files.write().clear();
    }
}

fn same(a: &Arc<str>, b: &Arc<str>) -> bool {
    Arc::ptr_eq(a, b) || a == b
}

/// A workspace function a call may call.
#[derive(Clone)]
pub(crate) struct Target {
    pub path: PathBuf,
    pub flow: Arc<FileFlow>,
    pub function: u32,
    pub via: CallEdgeKind,
}

/// Where an argument lands in a callee.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lands {
    /// The parameter at this position.
    Param(u32),
    /// No parameter takes it (an extra argument).
    Nowhere,
}

/// Where each argument of a call lands in one callee.
pub(crate) struct Landing {
    pub args: Vec<Lands>,
    /// The parameter the call's receiver is (Python's `self`).
    pub receiver: Option<u32>,
}

/// Summaries for one query: the shared cache when the host keeps one, and
/// the call chain being summarized.
pub(crate) struct Summaries<'h> {
    host: &'h dyn FlowHost,
    single: bool,
    cache: Option<&'h SummaryCache>,
    local: FxHashMap<Key, Arc<Summary>>,
    files: FxHashMap<PathBuf, Arc<FileInfo>>,
    active: FxHashSet<Key>,
}

impl<'h> Summaries<'h> {
    pub(crate) fn new(host: &'h dyn FlowHost) -> Self {
        Summaries {
            host,
            single: host.single_file(),
            cache: host.caller_index().map(|slot| slot.summaries()),
            local: FxHashMap::default(),
            files: FxHashMap::default(),
            active: FxHashSet::default(),
        }
    }

    fn info(&mut self, path: &Path, flow: &FileFlow) -> Arc<FileInfo> {
        if let Some(info) = self.files.get(path) {
            return Arc::clone(info);
        }
        let content = self.host.content(path);
        let cached = match (self.cache, &content) {
            (Some(cache), Some(content)) => cache
                .files
                .read()
                .get(path)
                .filter(|(then, _)| same(then, content))
                .map(|(_, info)| Arc::clone(info)),
            _ => None,
        };
        let info = cached.unwrap_or_else(|| {
            let info = Arc::new(FileInfo::of(flow));
            if let (Some(cache), Some(content)) = (self.cache, content) {
                cache
                    .files
                    .write()
                    .insert(path.to_path_buf(), (content, Arc::clone(&info)));
            }
            info
        });
        self.files.insert(path.to_path_buf(), Arc::clone(&info));
        info
    }

    /// Parameter bindings of a function.
    pub(crate) fn params(&mut self, path: &Path, flow: &FileFlow, function: u32) -> Vec<u32> {
        self.info(path, flow)
            .params
            .get(function as usize)
            .cloned()
            .unwrap_or_default()
    }

    /// The workspace functions a call may call (in file scope, only those
    /// in the same file). Not JSX, not callbacks.
    pub(crate) fn targets(
        &self,
        path: &Path,
        flow: &Arc<FileFlow>,
        call: &CallFact,
    ) -> Vec<Target> {
        if call.is_jsx {
            return Vec::new();
        }
        call_targets(self.host, path, flow, call)
            .into_iter()
            .filter(|(target, _, via)| {
                *via != CallEdgeKind::Callback && (!self.single || target == path)
            })
            .filter_map(|(target, function, via)| {
                let flow = self.host.flow(&target)?;
                Some(Target {
                    path: target,
                    flow,
                    function,
                    via,
                })
            })
            .collect()
    }

    /// The summary of a function, or None when it can't be had: it is being
    /// summarized already (recursion) or the call chain is too deep. A
    /// caller treats None as "may return anything it is given".
    pub(crate) fn of(&mut self, path: &Path, function: u32) -> Option<Arc<Summary>> {
        let key = (path.to_path_buf(), function);
        if let Some(found) = self.local.get(&key) {
            return Some(Arc::clone(found));
        }
        let content = self.host.content(path)?;
        if let Some(cache) = self.cache {
            let cached = cache
                .summaries
                .read()
                .get(&key)
                .filter(|(then, _)| same(then, &content))
                .map(|(_, summary)| Arc::clone(summary));
            if let Some(summary) = cached {
                self.local.insert(key, Arc::clone(&summary));
                return Some(summary);
            }
        }
        if self.active.contains(&key) || self.active.len() >= MAX_NESTED {
            return None;
        }
        let flow = self.host.flow(path)?;
        self.active.insert(key.clone());
        let summary = Arc::new(self.compute(path, &flow, function));
        self.active.remove(&key);
        if let Some(cache) = self.cache {
            cache
                .summaries
                .write()
                .insert(key.clone(), (content, Arc::clone(&summary)));
        }
        self.local.insert(key, Arc::clone(&summary));
        Some(summary)
    }

    /// Where each argument of `call` lands in `target`. None when it can't
    /// be told (a spread or unpacked argument, a bound method called as a
    /// plain function).
    pub(crate) fn landing(&mut self, call: &CallFact, target: &Target) -> Option<Landing> {
        let flow = &target.flow;
        let fact = flow.functions.get(target.function as usize)?;
        let method = flow.bound_self
            && fact.class_node.is_some()
            && !flow
                .decorators
                .get(&target.function)
                .is_some_and(|names| names.iter().any(|n| n.ends_with("staticmethod")));
        let offset = match (method, target.via) {
            (false, _) => 0,
            (true, CallEdgeKind::Method | CallEdgeKind::Type | CallEdgeKind::Constructor) => 1,
            (true, _) => return None,
        };
        let params = self.params(&target.path, flow, target.function);
        let param = |position: u32| {
            params.iter().copied().find(|&p| {
                flow.nodes[p as usize].index == Some(position) && !flow.variadic.contains_key(&p)
            })
        };
        let variadic = |kind: Variadic| {
            params
                .iter()
                .copied()
                .find(|p| flow.variadic.get(p) == Some(&kind))
                .and_then(|p| flow.nodes[p as usize].index)
        };
        let mut args = Vec::with_capacity(call.args.len());
        for position in 0..call.args.len() {
            let name: Option<&Name> = call.arg_names.get(position).and_then(Option::as_ref);
            let lands = match name.map(|n| &**n) {
                None => {
                    let at = position as u32 + offset;
                    match param(at) {
                        Some(_) => Lands::Param(at),
                        None => match variadic(Variadic::Positional) {
                            Some(rest) if rest <= at => Lands::Param(rest),
                            _ => Lands::Nowhere,
                        },
                    }
                }
                Some("..." | "*" | "**") => return None,
                Some(keyword) => {
                    let named = params.iter().copied().find(|&p| {
                        let node = &flow.nodes[p as usize];
                        node.name.as_deref() == Some(keyword)
                            && node.index.is_some_and(|i| i >= offset)
                            && !flow.variadic.contains_key(&p)
                    });
                    match named.and_then(|p| flow.nodes[p as usize].index) {
                        Some(at) => Lands::Param(at),
                        None => match variadic(Variadic::Keywords) {
                            Some(at) => Lands::Param(at),
                            None => Lands::Nowhere,
                        },
                    }
                }
            };
            args.push(lands);
        }
        let receiver = (method && target.via != CallEdgeKind::Constructor).then_some(0);
        Some(Landing { args, receiver })
    }

    /// What a call does with the value it gets at argument `position`
    /// (None: as the receiver): whether its result can carry it, and
    /// whether it is stored. Unknown callees carry everything.
    fn through_call(
        &mut self,
        path: &Path,
        flow: &Arc<FileFlow>,
        call: &CallFact,
        position: Option<usize>,
    ) -> ParamFlow {
        // A call we can't see into may return the value and may keep it.
        let unknown = ParamFlow {
            returns: true,
            stored: true,
        };
        let targets = self.targets(path, flow, call);
        if targets.is_empty() {
            return unknown;
        }
        let mut found = ParamFlow::default();
        for target in &targets {
            let Some(landing) = self.landing(call, target) else {
                return unknown;
            };
            let lands = match position {
                Some(position) => landing.args.get(position).copied(),
                None => landing.receiver.map(Lands::Param),
            };
            let Some(lands) = lands else {
                // A JavaScript receiver: `this` is not summarized.
                return unknown;
            };
            let Lands::Param(at) = lands else {
                continue;
            };
            let Some(summary) = self.of(&target.path, target.function) else {
                return unknown;
            };
            let flows = summary.param(at);
            found.returns |= flows.returns;
            found.stored |= flows.stored;
        }
        // `new C(x)`: the result is the instance, which may hold x.
        if call.is_new || targets.iter().any(|t| t.via == CallEdgeKind::Constructor) {
            found.returns = true;
        }
        found
    }

    fn compute(&mut self, path: &Path, flow: &Arc<FileFlow>, function: u32) -> Summary {
        let info = self.info(path, flow);
        let opaque = info.uses_arguments.contains(&function);
        let mut params: Vec<ParamFlow> = Vec::new();
        if opaque {
            return Summary { params, opaque };
        }
        for &binding in info.params.get(function as usize).into_iter().flatten() {
            let Some(position) = flow.nodes[binding as usize].index else {
                continue;
            };
            let flows = self.walk(path, flow, function, binding);
            let position = position as usize;
            if params.len() <= position {
                params.resize(position + 1, ParamFlow::default());
            }
            params[position].returns |= flows.returns;
            params[position].stored |= flows.stored;
        }
        Summary { params, opaque }
    }

    /// Where the value of `start` (a parameter of `function`) goes inside
    /// the function, nested functions included.
    fn walk(&mut self, path: &Path, flow: &Arc<FileFlow>, function: u32, start: u32) -> ParamFlow {
        let function_node = flow.functions[function as usize].node;
        let inside = |at: Option<u32>| {
            let mut current = at;
            while let Some(f) = current {
                if f == function {
                    return true;
                }
                current = flow.functions[f as usize].parent;
            }
            false
        };
        let mut found = ParamFlow::default();
        let mut seen = FxHashSet::from_iter([start]);
        let mut stack = vec![start];
        let mut push = |node: u32, stack: &mut Vec<u32>| {
            if seen.insert(node) {
                stack.push(node);
            }
        };
        while let Some(current) = stack.pop() {
            for &(other, edge) in &flow.out_edges[current as usize] {
                match edge {
                    FlowEdgeKind::Return => {
                        if other == function_node {
                            found.returns = true;
                        } else if flow
                            .fn_by_node
                            .get(&other)
                            .is_some_and(|&inner| inside(flow.functions[inner as usize].parent))
                        {
                            // Returned from a closure: the closure's value
                            // carries it, and so does each call to it here.
                            push(other, &mut stack);
                            let inner = flow.fn_by_node[&other];
                            for call in flow.calls.iter().filter(|c| inside(c.function)) {
                                let calls_it = self
                                    .targets(path, flow, call)
                                    .iter()
                                    .any(|t| t.function == inner && t.path == path);
                                if calls_it {
                                    if let Some(node) = flow.exact(call.range) {
                                        push(node, &mut stack);
                                    }
                                }
                            }
                        } else {
                            found.stored = true;
                        }
                    }
                    FlowEdgeKind::Store => {
                        // Into a local object or a parameter: that value now
                        // holds it. Into `this` or anything else: stored.
                        match store_root(flow, other) {
                            Some(root)
                                if inside(flow.nodes[root as usize].function)
                                    && matches!(
                                        flow.nodes[root as usize].kind,
                                        FlowNodeKind::Variable | FlowNodeKind::Parameter
                                    ) =>
                            {
                                if flow.nodes[root as usize].kind == FlowNodeKind::Parameter {
                                    found.stored = true;
                                }
                                push(root, &mut stack);
                            }
                            _ => found.stored = true,
                        }
                    }
                    FlowEdgeKind::Argument | FlowEdgeKind::Receiver => {
                        let Some(&call_index) = flow.call_by_node.get(&other) else {
                            push(other, &mut stack);
                            continue;
                        };
                        let call = &flow.calls[call_index as usize];
                        let positions: Vec<Option<usize>> = if edge == FlowEdgeKind::Receiver {
                            vec![None]
                        } else {
                            call.args
                                .iter()
                                .enumerate()
                                .filter(|(_, a)| **a == current)
                                .map(|(p, _)| Some(p))
                                .collect()
                        };
                        let mut carried = positions.is_empty();
                        for position in positions {
                            let flows = self.through_call(path, flow, call, position);
                            carried |= flows.returns;
                            found.stored |= flows.stored;
                        }
                        if carried {
                            push(other, &mut stack);
                        }
                    }
                    _ => {
                        let node = &flow.nodes[other as usize];
                        if inside(node.function) {
                            push(other, &mut stack);
                        } else {
                            // A variable outside the function.
                            found.stored = true;
                        }
                    }
                }
            }
        }
        found
    }
}

/// The binding whose object a property write writes into: `o` for
/// `o.p = v` and `o.a.b = v`. None for `this.p = v` and unknown targets.
fn store_root(flow: &FileFlow, member: u32) -> Option<u32> {
    let range = flow.nodes[member as usize].range;
    flow.mutations
        .iter()
        .filter(|m| {
            matches!(m.kind, MutationKind::PropertyWrite | MutationKind::Update)
                && range.is_within(&m.range)
        })
        .min_by_key(|m| m.range.len())
        .map(|m| m.binding)
}

/// A step a one-hop answer reports.
struct Hop {
    file: usize,
    node: u32,
    via: FlowEdgeKind,
    index: Option<u32>,
    boundary: bool,
    stop: bool,
    crossing: Option<Crossing>,
}

/// The steps one edge away from the node at `range`, each with depth 1:
/// the file's own edges, and with `across_calls` the steps across calls.
pub fn one_hop(
    host: &dyn FlowHost,
    path: &Path,
    range: ByteRange,
    options: FlowOptions,
) -> FlowResult {
    let path = host.canonical(path);
    let (Some(flow), Some(_)) = (host.flow(&path), host.content(&path)) else {
        return FlowResult::default();
    };
    let Some(start) = flow.locate(range) else {
        return FlowResult::default();
    };
    let backward = options.direction == FlowDirection::Backward;
    let mut files: Vec<(PathBuf, Arc<FileFlow>)> = vec![(path.clone(), Arc::clone(&flow))];
    let mut file_ids: HashMap<PathBuf, usize> = HashMap::from([(path.clone(), 0)]);
    let mut open = |p: &Path, f: &Arc<FileFlow>| -> usize {
        if let Some(&id) = file_ids.get(p) {
            return id;
        }
        files.push((p.to_path_buf(), Arc::clone(f)));
        file_ids.insert(p.to_path_buf(), files.len() - 1);
        files.len() - 1
    };
    let mut hops: Vec<Hop> = Vec::new();
    let mut summaries = options.across_calls.then(|| Summaries::new(host));
    let node = flow.nodes[start as usize].clone();

    // The call at the queried node, with its targets (backward only).
    let call_here = flow
        .call_by_node
        .get(&start)
        .map(|&c| &flow.calls[c as usize]);
    let mut backward_targets: Vec<(Target, Option<Landing>)> = Vec::new();
    if let (true, Some(call), Some(summaries)) = (backward, call_here, summaries.as_mut()) {
        for target in summaries.targets(&path, &flow, call) {
            let landing = summaries.landing(call, &target);
            backward_targets.push((target, landing));
        }
    }

    let edges = if backward {
        &flow.in_edges[start as usize]
    } else {
        &flow.out_edges[start as usize]
    };
    for &(other, edge) in edges {
        let call_edge = matches!(edge, FlowEdgeKind::Argument | FlowEdgeKind::Receiver);
        if backward && (edge == FlowEdgeKind::Return || (call_edge && !options.through_calls)) {
            continue;
        }
        if !options.through_calls {
            let pair = if backward {
                (other, start)
            } else {
                (start, other)
            };
            if flow.model_edges.contains(&pair) {
                continue;
            }
        }
        let target_node = &flow.nodes[other as usize];
        let (call_node, argument) = if backward {
            (start, other)
        } else {
            (other, start)
        };
        let call = flow
            .call_by_node
            .get(&call_node)
            .map(|&c| &flow.calls[c as usize]);
        let positions: Vec<usize> = match (edge, call) {
            (FlowEdgeKind::Argument, Some(call)) => call
                .args
                .iter()
                .enumerate()
                .filter(|(_, a)| **a == argument)
                .map(|(p, _)| p)
                .collect(),
            _ => Vec::new(),
        };
        let indexes: Vec<Option<u32>> = if positions.is_empty() {
            vec![if edge == FlowEdgeKind::Argument {
                None
            } else {
                target_node.index
            }]
        } else {
            positions.iter().map(|p| Some(*p as u32)).collect()
        };
        for index in indexes {
            // Stored or returned: the value leaves this walk here.
            let mut stop = !backward && matches!(edge, FlowEdgeKind::Return | FlowEdgeKind::Store);
            // Across calls, a call's result carries only what its callees'
            // summaries say comes back out.
            if let (true, Some(call), Some(summaries)) = (call_edge, call, summaries.as_mut()) {
                let position = if edge == FlowEdgeKind::Receiver {
                    None
                } else {
                    index.map(|i| i as usize)
                };
                let flows = summaries.through_call(&path, &flow, call, position);
                if !flows.returns {
                    if backward {
                        continue;
                    }
                    stop = true;
                }
            }
            hops.push(Hop {
                file: 0,
                node: other,
                via: edge,
                index,
                boundary: backward && is_boundary(target_node.kind),
                stop,
                crossing: None,
            });
        }

        // Forward across calls: into the callee's parameter; a returned
        // value out to the function's call sites.
        let Some(summaries) = summaries.as_mut() else {
            continue;
        };
        if backward {
            continue;
        }
        match (edge, call) {
            (FlowEdgeKind::Argument | FlowEdgeKind::Receiver, Some(call)) => {
                for target in summaries.targets(&path, &flow, call) {
                    let Some(landing) = summaries.landing(call, &target) else {
                        continue;
                    };
                    let lands: Vec<Lands> = if edge == FlowEdgeKind::Receiver {
                        landing.receiver.map(Lands::Param).into_iter().collect()
                    } else {
                        positions.iter().map(|p| landing.args[*p]).collect()
                    };
                    for lands in lands {
                        let Lands::Param(at) = lands else { continue };
                        let file = open(&target.path, &target.flow);
                        for param in summaries.params(&target.path, &target.flow, target.function) {
                            if target.flow.nodes[param as usize].index == Some(at) {
                                hops.push(Hop {
                                    file,
                                    node: param,
                                    via: FlowEdgeKind::Parameter,
                                    index: Some(at),
                                    boundary: false,
                                    stop: false,
                                    crossing: Some(Crossing::Into),
                                });
                            }
                        }
                    }
                }
            }
            (FlowEdgeKind::Return, _) => {
                let Some(&function) = flow.fn_by_node.get(&other) else {
                    continue;
                };
                for (site_path, call_index, _) in call_sites_of(host, &path, function) {
                    let Some(site_flow) = host.flow(&site_path) else {
                        continue;
                    };
                    let Some(site) = site_flow
                        .calls
                        .get(call_index as usize)
                        .and_then(|call| site_flow.exact(call.range))
                    else {
                        continue;
                    };
                    let file = open(&site_path, &site_flow);
                    hops.push(Hop {
                        file,
                        node: site,
                        via: FlowEdgeKind::Return,
                        index: None,
                        boundary: false,
                        stop: false,
                        crossing: Some(Crossing::Out),
                    });
                }
            }
            _ => {}
        }
    }

    // Backward across calls: a call's result to its callees' returned
    // values; a parameter to its callers' arguments.
    if let (true, Some(summaries)) = (backward, summaries.as_mut()) {
        let constructor = call_here.is_some_and(|c| c.is_new)
            || backward_targets
                .iter()
                .any(|(t, _)| t.via == CallEdgeKind::Constructor);
        if !constructor {
            for (target, _) in &backward_targets {
                let function_node = target.flow.functions[target.function as usize].node;
                let file = open(&target.path, &target.flow);
                for &(value, edge) in &target.flow.in_edges[function_node as usize] {
                    if edge == FlowEdgeKind::Return {
                        hops.push(Hop {
                            file,
                            node: value,
                            via: FlowEdgeKind::Return,
                            index: None,
                            boundary: is_boundary(target.flow.nodes[value as usize].kind),
                            stop: false,
                            crossing: Some(Crossing::Into),
                        });
                    }
                }
            }
        }
        if let (true, FlowNodeKind::Parameter, Some(function), Some(position)) =
            (node.binding, node.kind, node.function, node.index)
        {
            for (site_path, call_index, via) in call_sites_of(host, &path, function) {
                let Some(site_flow) = host.flow(&site_path) else {
                    continue;
                };
                let Some(call) = site_flow.calls.get(call_index as usize) else {
                    continue;
                };
                let target = Target {
                    path: path.clone(),
                    flow: Arc::clone(&flow),
                    function,
                    via,
                };
                let Some(landing) = summaries.landing(call, &target) else {
                    continue;
                };
                let mut arguments: Vec<(u32, Option<u32>)> = landing
                    .args
                    .iter()
                    .enumerate()
                    .filter(|(_, lands)| **lands == Lands::Param(position))
                    .map(|(p, _)| (call.args[p], Some(position)))
                    .collect();
                if landing.receiver == Some(position) {
                    if let crate::facts::CalleeFact::Member { object, .. } = &call.callee {
                        arguments.push((*object, Some(position)));
                    }
                }
                if arguments.is_empty() {
                    continue;
                }
                let file = open(&site_path, &site_flow);
                for (argument, index) in arguments {
                    hops.push(Hop {
                        file,
                        node: argument,
                        via: FlowEdgeKind::Parameter,
                        index,
                        boundary: is_boundary(site_flow.nodes[argument as usize].kind),
                        stop: false,
                        crossing: Some(Crossing::Out),
                    });
                }
            }
        }
    }

    // A parameter with callers in scope, or a call into the workspace, is
    // not where the value stops any more.
    if let Some(summaries) = summaries.as_ref() {
        for hop in hops.iter_mut().filter(|h| h.boundary) {
            let hop_flow = &files[hop.file].1;
            let hop_path = files[hop.file].0.clone();
            let n = &hop_flow.nodes[hop.node as usize];
            hop.boundary = match n.kind {
                FlowNodeKind::Parameter => match (n.binding, n.function) {
                    (true, Some(function)) => call_sites_of(host, &hop_path, function).is_empty(),
                    _ => true,
                },
                FlowNodeKind::Call => match hop_flow.call_by_node.get(&hop.node) {
                    Some(&c) => summaries
                        .targets(&hop_path, hop_flow, &hop_flow.calls[c as usize])
                        .is_empty(),
                    None => true,
                },
                _ => true,
            };
        }
    }

    let mut steps: Vec<Vec<FlowStep>> = vec![Vec::new(); files.len()];
    let mut seen: FxHashSet<(usize, u32, FlowEdgeKind, Option<u32>)> = FxHashSet::default();
    for hop in hops {
        if !seen.insert((hop.file, hop.node, hop.via, hop.index)) {
            continue;
        }
        let n = &files[hop.file].1.nodes[hop.node as usize];
        steps[hop.file].push(FlowStep {
            range: n.range,
            kind: n.kind,
            via: hop.via,
            name: n.name.as_deref().map(str::to_string),
            index: hop.index,
            depth: 1,
            boundary: hop.boundary,
            from: None,
            from_file: None,
            stop: hop.stop,
            crossing: hop.crossing,
        });
    }
    FlowResult {
        files: files
            .into_iter()
            .zip(steps)
            .enumerate()
            .filter(|(i, (_, steps))| *i == 0 || !steps.is_empty())
            .filter_map(|(_, ((file_path, _), steps))| {
                let content = host.content(&file_path)?;
                Some(FileFlowSteps {
                    file_path,
                    content: content.to_string(),
                    steps,
                })
            })
            .collect(),
        truncated: false,
    }
}

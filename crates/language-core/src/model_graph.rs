//! The model facts of a workspace, joined into a graph every provider can
//! merge into its answers, and the answers a provider with no call graph or
//! data flow of its own can still give from the facts alone.
//!
//! One [`ModelStore`] per workspace, shared by the providers of every
//! language: a TypeScript `fetch` and the Python route that answers it are
//! joined here, whichever provider is asked.

use crate::models::{
    match_keys, ChannelFact, ChannelReport, ChannelSide, ModelFact, ModelFileFacts, ModelNode,
};
use crate::{
    ByteRange, CallEdgeKind, CallGraphDirection, CallGraphEntry, CallGraphFile, CallGraphOptions,
    CallGraphResult, FileFlowSteps, FlowDirection, FlowEdgeKind, FlowNodeKind, FlowOptions,
    FlowResult, FlowStep, FunctionRef,
};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// A value: its file and range.
pub type ModelValue = (PathBuf, ByteRange);

/// The step (file index, step index) a walk reached a value from.
type Reached = Option<(u32, u32)>;

/// A function a fact is about.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelFunction {
    pub file: PathBuf,
    pub range: ByteRange,
    pub name: Option<String>,
}

impl ModelFunction {
    pub fn key(&self) -> ModelValue {
        (self.file.clone(), self.range)
    }
}

/// How a provider reads a fact's nodes in files it can analyze. Each answer
/// may be None ("not my file"): the graph then takes what the fact itself
/// says (the node's range, and the function the toolkit found around it).
pub trait ModelResolver {
    /// The path the provider names the file by.
    fn canonical(&self, path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }
    /// The functions the node stands for: the function, its name, its
    /// export statement, a reference to it.
    fn functions(&self, _node: &ModelNode) -> Option<Vec<ModelFunction>> {
        None
    }
    /// The function a call site is in. `Some(None)`: module top level.
    fn enclosing(&self, _node: &ModelNode) -> Option<Option<ModelFunction>> {
        None
    }
    /// The range of the value the node stands for.
    fn value(&self, _node: &ModelNode) -> Option<ByteRange> {
        None
    }
}

/// Reads nodes as the facts give them. For languages whose provider has no
/// functions or values of its own to offer.
pub struct FactResolver;

impl ModelResolver for FactResolver {}

/// A call a model added.
#[derive(Debug, Clone)]
pub struct ModelCall {
    /// Where the call happens: a call expression, or a function.
    pub site: ModelValue,
    /// The function the site is in. None: module top level.
    pub from: Option<ModelFunction>,
    pub to: ModelFunction,
    pub via: CallEdgeKind,
    pub models: Vec<String>,
    pub channel: Option<String>,
}

/// A value-flow edge a model added.
#[derive(Debug, Clone)]
pub struct ModelFlow {
    pub from: ModelValue,
    pub to: ModelValue,
    pub via: FlowEdgeKind,
    pub models: Vec<String>,
    pub channel: Option<String>,
}

/// "Code outside the workspace calls this."
#[derive(Debug, Clone)]
pub struct ModelEntryPoint {
    pub kind: String,
    pub label: Option<String>,
    pub model: String,
}

/// The facts, resolved and joined.
#[derive(Debug, Default)]
pub struct ModelGraph {
    pub calls: Vec<ModelCall>,
    calls_to: HashMap<ModelValue, Vec<u32>>,
    calls_from: HashMap<PathBuf, Vec<u32>>,
    pub flows: Vec<ModelFlow>,
    flows_from: HashMap<ModelValue, Vec<u32>>,
    flows_to: HashMap<ModelValue, Vec<u32>>,
    entry_points: HashMap<ModelValue, Vec<ModelEntryPoint>>,
    /// Every function a fact names, by file.
    functions: HashMap<PathBuf, Vec<ModelFunction>>,
    tags: HashMap<ModelValue, Vec<(String, String)>>,
    pub channels: Vec<ChannelReport>,
}

impl ModelGraph {
    pub fn calls_to(&self, function: &ModelValue) -> impl Iterator<Item = (u32, &ModelCall)> {
        self.calls_to
            .get(function)
            .into_iter()
            .flatten()
            .map(|index| (*index, &self.calls[*index as usize]))
    }

    /// Calls made from somewhere in `file`.
    pub fn calls_from(&self, file: &Path) -> impl Iterator<Item = (u32, &ModelCall)> {
        self.calls_from
            .get(file)
            .into_iter()
            .flatten()
            .map(|index| (*index, &self.calls[*index as usize]))
    }

    /// Edges out of a value (forward) or into it (backward).
    pub fn flows_at(&self, value: &ModelValue, backward: bool) -> impl Iterator<Item = &ModelFlow> {
        let table = if backward {
            &self.flows_to
        } else {
            &self.flows_from
        };
        table
            .get(value)
            .into_iter()
            .flatten()
            .map(|index| &self.flows[*index as usize])
    }

    pub fn entry_points(&self, function: &ModelValue) -> &[ModelEntryPoint] {
        self.entry_points.get(function).map_or(&[], Vec::as_slice)
    }

    /// True when a model says something calls the function.
    pub fn explains(&self, function: &ModelValue) -> bool {
        self.entry_points.contains_key(function) || self.calls_to.contains_key(function)
    }

    /// (label, model) pairs on a value.
    pub fn tags_at(&self, value: &ModelValue) -> &[(String, String)] {
        self.tags.get(value).map_or(&[], Vec::as_slice)
    }

    /// The function facts name at `range`: that range, else the smallest
    /// one around it, else the largest one inside it.
    pub fn function_at(&self, file: &Path, range: ByteRange) -> Option<&ModelFunction> {
        let functions = self.functions.get(file)?;
        let size = |f: &&ModelFunction| f.range.end - f.range.start;
        functions
            .iter()
            .find(|f| f.range == range)
            .or_else(|| {
                functions
                    .iter()
                    .filter(|f| range.is_within(&f.range))
                    .min_by_key(size)
            })
            .or_else(|| {
                functions
                    .iter()
                    .filter(|f| f.range.is_within(&range))
                    .max_by_key(size)
            })
    }

    fn remember(&mut self, function: &ModelFunction) {
        let known = self.functions.entry(function.file.clone()).or_default();
        if !known.iter().any(|f| f.range == function.range) {
            known.push(function.clone());
        }
    }

    fn add_call(&mut self, call: ModelCall) {
        let index = self.calls.len() as u32;
        self.remember(&call.to);
        if let Some(from) = &call.from {
            self.remember(from);
        }
        self.calls_to.entry(call.to.key()).or_default().push(index);
        self.calls_from
            .entry(call.site.0.clone())
            .or_default()
            .push(index);
        self.calls.push(call);
    }

    fn add_flow(&mut self, flow: ModelFlow) {
        let index = self.flows.len() as u32;
        self.flows_from
            .entry(flow.from.clone())
            .or_default()
            .push(index);
        self.flows_to
            .entry(flow.to.clone())
            .or_default()
            .push(index);
        self.flows.push(flow);
    }

    /// Resolve the facts' nodes with `resolver` and join the channels.
    pub fn build(
        facts: &BTreeMap<(String, PathBuf), Vec<ModelFact>>,
        resolver: &dyn ModelResolver,
    ) -> Self {
        let mut graph = ModelGraph::default();
        let mut ends: Vec<(&str, &ChannelFact)> = Vec::new();
        for ((model, _), file_facts) in facts {
            for fact in file_facts {
                match fact {
                    ModelFact::EntryPoint {
                        node,
                        entry_kind,
                        label,
                    } => {
                        for function in functions(resolver, node) {
                            graph.remember(&function);
                            graph.entry_points.entry(function.key()).or_default().push(
                                ModelEntryPoint {
                                    kind: entry_kind.clone(),
                                    label: label.clone(),
                                    model: model.clone(),
                                },
                            );
                        }
                    }
                    ModelFact::Invokes { from, to } => {
                        let (site, from) = call_site(resolver, from);
                        for to in functions(resolver, to) {
                            graph.add_call(ModelCall {
                                site: site.clone(),
                                from: from.clone(),
                                to,
                                via: CallEdgeKind::Model,
                                models: vec![model.clone()],
                                channel: None,
                            });
                        }
                    }
                    ModelFact::Flows { from, to } => graph.add_flow(ModelFlow {
                        from: value(resolver, from),
                        to: value(resolver, to),
                        via: FlowEdgeKind::Model,
                        models: vec![model.clone()],
                        channel: None,
                    }),
                    ModelFact::Channel(channel) => ends.push((model, channel)),
                    ModelFact::Tag { node, labels } => {
                        let tags = graph.tags.entry(value(resolver, node)).or_default();
                        for label in labels {
                            let tag = (label.clone(), model.clone());
                            if !tags.contains(&tag) {
                                tags.push(tag);
                            }
                        }
                    }
                }
            }
        }
        join_channels(resolver, &ends, &mut graph);
        graph
    }
}

fn value(resolver: &dyn ModelResolver, node: &ModelNode) -> ModelValue {
    (
        resolver.canonical(&node.file),
        resolver.value(node).unwrap_or_else(|| node.range()),
    )
}

fn functions(resolver: &dyn ModelResolver, node: &ModelNode) -> Vec<ModelFunction> {
    resolver.functions(node).unwrap_or_else(|| {
        vec![ModelFunction {
            file: resolver.canonical(&node.file),
            range: node.range(),
            name: node.name.clone(),
        }]
    })
}

fn call_site(
    resolver: &dyn ModelResolver,
    node: &ModelNode,
) -> (ModelValue, Option<ModelFunction>) {
    let file = resolver.canonical(&node.file);
    let from = resolver.enclosing(node).unwrap_or_else(|| {
        node.within.as_ref().map(|within| ModelFunction {
            file: file.clone(),
            range: ByteRange::new(within.start, within.end),
            name: within.name.clone(),
        })
    });
    ((file, node.range()), from)
}

/// Join every producer with the consumers its key reaches: a call edge from
/// the producer to the consumer's function, and a value-flow edge for each
/// pair of ports with the same name (or joined by position in the key).
fn join_channels(
    resolver: &dyn ModelResolver,
    ends: &[(&str, &ChannelFact)],
    graph: &mut ModelGraph,
) {
    let mut matches = vec![0u32; ends.len()];
    for (producer_index, (producer_model, producer)) in ends.iter().enumerate() {
        if producer.side != ChannelSide::Producer || producer.key.is_open() {
            continue;
        }
        for (consumer_index, (consumer_model, consumer)) in ends.iter().enumerate() {
            if consumer.side != ChannelSide::Consumer || consumer.key.is_open() {
                continue;
            }
            let Some(found) = match_keys(&producer.key, &consumer.key) else {
                continue;
            };
            matches[producer_index] += 1;
            matches[consumer_index] += 1;
            let mut models = vec![producer_model.to_string()];
            if producer_model != consumer_model {
                models.push(consumer_model.to_string());
            }
            let channel = Some(consumer.key.display());

            let (site, from) = call_site(resolver, &producer.node);
            for to in functions(resolver, &consumer.node) {
                graph.add_call(ModelCall {
                    site: site.clone(),
                    from: from.clone(),
                    to,
                    via: CallEdgeKind::Channel,
                    models: models.clone(),
                    channel: channel.clone(),
                });
            }

            let mut connect =
                |sender: &ChannelFact, sent: &str, receiver: &ChannelFact, got: &str| {
                    for from in sender.sends.iter().filter(|port| port.name == sent) {
                        for to in receiver.receives.iter().filter(|port| port.name == got) {
                            graph.add_flow(ModelFlow {
                                from: value(resolver, &from.node),
                                to: value(resolver, &to.node),
                                via: FlowEdgeKind::Channel,
                                models: models.clone(),
                                channel: channel.clone(),
                            });
                        }
                    }
                };
            for (sent, got) in &found.bindings {
                connect(producer, sent, consumer, got);
            }
            // Ports with the same name on both ends, each way (request
            // body one way, response the other).
            for port in &producer.sends {
                if !found.bindings.iter().any(|(sent, _)| sent == &port.name) {
                    connect(producer, &port.name, consumer, &port.name);
                }
            }
            for port in &consumer.sends {
                connect(consumer, &port.name, producer, &port.name);
            }
        }
    }
    for (index, (model, end)) in ends.iter().enumerate() {
        graph.channels.push(ChannelReport {
            model: model.to_string(),
            side: end.side,
            protocol: end.key.protocol.clone(),
            key: end.key.display(),
            node: ModelNode {
                file: resolver.canonical(&end.node.file),
                ..end.node.clone()
            },
            open: end.key.is_open(),
            matches: matches[index],
        });
    }
}

/// The facts of every model for one workspace.
#[derive(Default)]
pub struct ModelStore {
    /// By (model, file the model ran on).
    facts: RwLock<BTreeMap<(String, PathBuf), Vec<ModelFact>>>,
    version: AtomicU64,
    fact_graph: ModelGraphCache,
}

impl ModelStore {
    /// The store of the workspace at `root`: the same one for every
    /// provider (of any language) made for that root in this process.
    pub fn for_workspace(root: &Path) -> Arc<ModelStore> {
        static STORES: OnceLock<Mutex<HashMap<PathBuf, Arc<ModelStore>>>> = OnceLock::new();
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let mut stores = STORES
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Arc::clone(stores.entry(root).or_default())
    }

    fn facts(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, BTreeMap<(String, PathBuf), Vec<ModelFact>>> {
        self.facts
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Replace what `model` stated about `file`.
    pub fn set(&self, model: &str, file: PathBuf, facts: Vec<ModelFact>) {
        let key = (model.to_string(), file);
        let mut all = self.facts();
        if facts.is_empty() {
            all.remove(&key);
        } else {
            all.insert(key, facts);
        }
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    pub fn clear(&self, model: Option<&str>) {
        let mut all = self.facts();
        match model {
            Some(model) => all.retain(|(name, _), _| name != model),
            None => all.clear(),
        }
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    pub fn all(&self) -> Vec<ModelFileFacts> {
        self.facts()
            .iter()
            .map(|((model, file), facts)| ModelFileFacts {
                model: model.clone(),
                file: file.clone(),
                facts: facts.clone(),
            })
            .collect()
    }

    /// Changes with every `set` and `clear`.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// The graph as the facts alone give it ([`FactResolver`]).
    pub fn fact_graph(&self) -> Option<Arc<ModelGraph>> {
        self.fact_graph.get(self, &FactResolver)
    }

    /// The graph as `resolver` reads it. None when no model stated anything.
    pub fn graph(&self, resolver: &dyn ModelResolver) -> Option<ModelGraph> {
        let facts = self.facts().clone();
        if facts.is_empty() {
            return None;
        }
        Some(ModelGraph::build(&facts, resolver))
    }
}

impl std::fmt::Debug for ModelStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelStore")
            .field("version", &self.version())
            .finish()
    }
}

/// A provider's graph, rebuilt when the store changes.
#[derive(Debug, Default)]
pub struct ModelGraphCache(RwLock<Option<(u64, Option<Arc<ModelGraph>>)>>);

impl ModelGraphCache {
    pub fn get(&self, store: &ModelStore, resolver: &dyn ModelResolver) -> Option<Arc<ModelGraph>> {
        let version = store.version();
        if let Ok(cached) = self.0.read() {
            if let Some((built, graph)) = cached.as_ref() {
                if *built == version {
                    return graph.clone();
                }
            }
        }
        let graph = store.graph(resolver).map(Arc::new);
        if let Ok(mut cached) = self.0.write() {
            *cached = Some((version, graph.clone()));
        }
        graph
    }
}

/// Call-graph entries grouped by file, in the order found.
#[derive(Default)]
struct Found {
    files: Vec<(PathBuf, Vec<CallGraphEntry>)>,
}

impl Found {
    fn push(&mut self, file: &Path, entry: CallGraphEntry) {
        match self.files.iter_mut().find(|(path, _)| path == file) {
            Some((_, entries)) => entries.push(entry),
            None => self.files.push((file.to_path_buf(), vec![entry])),
        }
    }
}

/// `callers()` / `callees()` from the facts alone: the edges models added,
/// and nothing a provider would find in the code. For a language whose
/// provider has no call graph.
pub fn model_call_graph(
    graph: &ModelGraph,
    file: &Path,
    range: ByteRange,
    options: CallGraphOptions,
) -> CallGraphResult {
    let Some(start) = graph.function_at(file, range) else {
        return CallGraphResult::default();
    };
    let callers = options.direction == CallGraphDirection::Callers;
    let mut found = Found::default();
    let mut visited: HashSet<ModelValue> = HashSet::from([start.key()]);
    let mut reported: HashSet<u32> = HashSet::new();
    let mut queue: VecDeque<(ModelFunction, u32)> = VecDeque::from([(start.clone(), 0)]);
    while let Some((function, depth)) = queue.pop_front() {
        if options.max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        if callers {
            let callee = FunctionRef {
                file_path: function.file.clone(),
                range: function.range,
                name: function.name.clone(),
            };
            for entry_point in graph.entry_points(&function.key()) {
                found.push(
                    &function.file,
                    CallGraphEntry {
                        range: function.range,
                        name: Some(format!("<entry-point:{}>", entry_point.kind)),
                        resolved: true,
                        via: CallEdgeKind::EntryPoint,
                        depth: depth + 1,
                        call_site: Some(function.key()),
                        callee: Some(callee.clone()),
                        models: vec![entry_point.model.clone()],
                        channel: entry_point.label.clone(),
                    },
                );
            }
            for (index, call) in graph.calls_to(&function.key()) {
                if !reported.insert(index) {
                    continue;
                }
                found.push(
                    &call.site.0,
                    CallGraphEntry {
                        range: call.from.as_ref().map_or(call.site.1, |from| from.range),
                        name: Some(match &call.from {
                            None => "<module>".to_string(),
                            Some(from) => from.name.clone().unwrap_or("<anonymous>".to_string()),
                        }),
                        resolved: true,
                        via: call.via,
                        depth: depth + 1,
                        call_site: Some(call.site.clone()),
                        callee: Some(callee.clone()),
                        models: call.models.clone(),
                        channel: call.channel.clone(),
                    },
                );
                if let Some(from) = &call.from {
                    if options.transitive
                        && !options.stop_at.contains(call.via)
                        && visited.insert(from.key())
                    {
                        queue.push_back((from.clone(), depth + 1));
                    }
                }
            }
        } else {
            for (index, call) in graph.calls_from(&function.file) {
                let inside = call
                    .from
                    .as_ref()
                    .is_some_and(|from| from.range == function.range);
                if !inside || !reported.insert(index) {
                    continue;
                }
                found.push(
                    &call.to.file,
                    CallGraphEntry {
                        range: call.to.range,
                        name: call.to.name.clone(),
                        resolved: true,
                        via: call.via,
                        depth: depth + 1,
                        call_site: Some(call.site.clone()),
                        callee: None,
                        models: call.models.clone(),
                        channel: call.channel.clone(),
                    },
                );
                if options.transitive
                    && !options.stop_at.contains(call.via)
                    && visited.insert(call.to.key())
                {
                    queue.push_back((call.to.clone(), depth + 1));
                }
            }
        }
    }
    CallGraphResult {
        files: found
            .files
            .into_iter()
            .filter_map(|(file_path, entries)| {
                Some(CallGraphFile {
                    content: std::fs::read_to_string(&file_path).ok()?,
                    file_path,
                    entries,
                })
            })
            .collect(),
        truncated: false,
        unparsed: Vec::new(),
    }
}

/// `flowsTo()` / `flowsFrom()` from the facts alone: only the edges models
/// added, starting from a node a fact names (a channel port, a `flows` end).
pub fn model_data_flow(
    graph: &ModelGraph,
    file: &Path,
    range: ByteRange,
    options: FlowOptions,
) -> FlowResult {
    let backward = options.direction == FlowDirection::Backward;
    let mut files: Vec<(PathBuf, Vec<FlowStep>)> = vec![(file.to_path_buf(), Vec::new())];
    let mut seen: HashSet<ModelValue> = HashSet::from([(file.to_path_buf(), range)]);
    // Value, depth, and the step (file, index) that reached it.
    let mut queue: VecDeque<(ModelValue, u32, Reached)> =
        VecDeque::from([((file.to_path_buf(), range), 0, None)]);
    while let Some((here, depth, reached_by)) = queue.pop_front() {
        if options.max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        for edge in graph.flows_at(&here, backward) {
            let other = if backward { &edge.from } else { &edge.to };
            if !seen.insert(other.clone()) {
                continue;
            }
            let file_index = match files.iter().position(|(path, _)| path == &other.0) {
                Some(index) => index,
                None => {
                    files.push((other.0.clone(), Vec::new()));
                    files.len() - 1
                }
            } as u32;
            let steps = &mut files[file_index as usize].1;
            steps.push(FlowStep {
                range: other.1,
                kind: FlowNodeKind::Expression,
                via: edge.via,
                name: None,
                index: None,
                depth: depth + 1,
                boundary: backward,
                from: reached_by.map(|(_, step)| step),
                from_file: reached_by.map(|(f, _)| f).filter(|f| *f != file_index),
                models: edge.models.clone(),
                channel: edge.channel.clone(),
                tags: graph
                    .tags_at(other)
                    .iter()
                    .map(|(label, _)| label.clone())
                    .collect(),
            });
            let step = steps.len() as u32 - 1;
            if !options.stop_at.contains(edge.via) {
                queue.push_back((other.clone(), depth + 1, Some((file_index, step))));
            }
        }
    }
    FlowResult {
        files: files
            .into_iter()
            .filter_map(|(file_path, steps)| {
                Some(FileFlowSteps {
                    content: std::fs::read_to_string(&file_path).ok()?,
                    file_path,
                    steps,
                })
            })
            .collect(),
        truncated: false,
    }
}

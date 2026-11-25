//! Model facts held by the workspace analyzer, and the graph the queries
//! merge into their answers.
//!
//! Facts come from outside (see `language_core::ModelFact`): per model, per
//! file. Nothing here knows a framework. The store keeps the facts as given;
//! the first query after a change turns them into a [`ModelGraph`]: nodes
//! resolved to functions and value-flow nodes, and channel producers joined
//! with consumers.

use crate::flow_query::{functions_at, FlowHost};
use language_core::{
    match_keys, ByteRange, CallEdgeKind, ChannelFact, ChannelReport, ChannelSide, FlowEdgeKind,
    ModelFact, ModelFileFacts, ModelNode,
};
use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A function: its file and its index in the file's facts.
type Function = (PathBuf, u32);
/// A value: its file and its range in the file's facts.
type Value = (PathBuf, ByteRange);

/// A call a model added.
pub(crate) struct ModelCall {
    /// Where the call happens: a call expression, or a function.
    pub site: Value,
    /// The function the site is in (the function itself when the site is
    /// one). None: module top level.
    pub from: Option<u32>,
    pub to: Function,
    pub via: CallEdgeKind,
    pub models: Vec<String>,
    pub channel: Option<String>,
}

/// A value-flow edge a model added.
pub(crate) struct ModelFlow {
    pub from: Value,
    pub to: Value,
    pub via: FlowEdgeKind,
    pub models: Vec<String>,
    pub channel: Option<String>,
}

/// "Code outside the workspace calls this."
pub(crate) struct ModelEntry {
    pub kind: String,
    pub label: Option<String>,
    pub model: String,
}

/// The facts, resolved and joined.
#[derive(Default)]
pub(crate) struct ModelGraph {
    pub calls: Vec<ModelCall>,
    calls_to: HashMap<Function, Vec<u32>>,
    calls_from: HashMap<PathBuf, Vec<u32>>,
    pub flows: Vec<ModelFlow>,
    flows_from: HashMap<Value, Vec<u32>>,
    flows_to: HashMap<Value, Vec<u32>>,
    entries: HashMap<Function, Vec<ModelEntry>>,
    tags: HashMap<Value, Vec<(String, String)>>,
    pub channels: Vec<ChannelReport>,
}

impl ModelGraph {
    pub fn calls_to(&self, function: &Function) -> impl Iterator<Item = (u32, &ModelCall)> {
        self.calls_to
            .get(function)
            .into_iter()
            .flatten()
            .map(|index| (*index, &self.calls[*index as usize]))
    }

    /// Calls made from somewhere in `file`.
    pub fn calls_from(&self, file: &Path) -> impl Iterator<Item = &ModelCall> {
        self.calls_from
            .get(file)
            .into_iter()
            .flatten()
            .map(|index| &self.calls[*index as usize])
    }

    /// Edges out of a value (forward) or into it (backward).
    pub fn flows_at(&self, value: &Value, backward: bool) -> impl Iterator<Item = &ModelFlow> {
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

    pub fn entries_of(&self, function: &Function) -> &[ModelEntry] {
        self.entries.get(function).map_or(&[], Vec::as_slice)
    }

    /// True when a model says something calls the function.
    pub fn explains(&self, function: &Function) -> bool {
        self.entries.contains_key(function) || self.calls_to.contains_key(function)
    }

    /// (label, model) pairs on a value.
    pub fn tags_at(&self, value: &Value) -> &[(String, String)] {
        self.tags.get(value).map_or(&[], Vec::as_slice)
    }

    fn add_call(&mut self, call: ModelCall) {
        let index = self.calls.len() as u32;
        self.calls_to
            .entry(call.to.clone())
            .or_default()
            .push(index);
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
}

/// The value a fact's node stands for: its file as the analyzer names it,
/// and the range of the value-flow node there (a fact's range comes from
/// another parser and may sit a token wider).
fn value(host: &dyn FlowHost, node: &ModelNode) -> Value {
    let path = host.canonical(&node.file);
    let range = host
        .flow(&path)
        .and_then(|flow| {
            let found = flow.locate(node.range())?;
            Some(flow.nodes[found as usize].range)
        })
        .unwrap_or_else(|| node.range());
    (path, range)
}

/// The functions a fact's node stands for: the function, its name, its
/// export statement, or a reference to it.
fn functions(host: &dyn FlowHost, node: &ModelNode) -> Vec<Function> {
    let path = host.canonical(&node.file);
    functions_at(host, &path, node.range(), true)
        .into_iter()
        .filter_map(|(path, function)| Some((path, function?)))
        .collect()
}

/// Where a call a model added is made from: the site and its function.
fn call_site(host: &dyn FlowHost, node: &ModelNode) -> (Value, Option<u32>) {
    let path = host.canonical(&node.file);
    let range = node.range();
    let from = host.flow(&path).and_then(|flow| {
        flow.functions
            .iter()
            .position(|f| f.range == range || f.name_range == Some(range))
            .map(|index| index as u32)
            .or_else(|| flow.function_at(range))
    });
    ((path, range), from)
}

fn build(host: &dyn FlowHost, facts: &BTreeMap<(String, PathBuf), Vec<ModelFact>>) -> ModelGraph {
    let mut graph = ModelGraph::default();
    let mut ends: Vec<(&str, &ChannelFact)> = Vec::new();

    for ((model, _), file_facts) in facts {
        for fact in file_facts {
            match fact {
                ModelFact::Entry {
                    node,
                    entry_kind,
                    label,
                } => {
                    for function in functions(host, node) {
                        graph.entries.entry(function).or_default().push(ModelEntry {
                            kind: entry_kind.clone(),
                            label: label.clone(),
                            model: model.clone(),
                        });
                    }
                }
                ModelFact::Invokes { from, to } => {
                    let (site, from) = call_site(host, from);
                    for to in functions(host, to) {
                        graph.add_call(ModelCall {
                            site: site.clone(),
                            from,
                            to,
                            via: CallEdgeKind::Model,
                            models: vec![model.clone()],
                            channel: None,
                        });
                    }
                }
                ModelFact::Flows { from, to } => graph.add_flow(ModelFlow {
                    from: value(host, from),
                    to: value(host, to),
                    via: FlowEdgeKind::Model,
                    models: vec![model.clone()],
                    channel: None,
                }),
                ModelFact::Channel(channel) => ends.push((model, channel)),
                ModelFact::Tag { node, labels } => {
                    let tags = graph.tags.entry(value(host, node)).or_default();
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

    join_channels(host, &ends, &mut graph);
    graph
}

/// Join every producer with the consumers its key reaches: a call edge from
/// the producer to the consumer's function, and a value-flow edge for each
/// pair of ports with the same name (or joined by position in the key).
fn join_channels(host: &dyn FlowHost, ends: &[(&str, &ChannelFact)], graph: &mut ModelGraph) {
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

            let (site, from) = call_site(host, &producer.node);
            for to in functions(host, &consumer.node) {
                graph.add_call(ModelCall {
                    site: site.clone(),
                    from,
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
                                from: value(host, &from.node),
                                to: value(host, &to.node),
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
                file: host.canonical(&end.node.file),
                ..end.node.clone()
            },
            open: end.key.is_open(),
            matches: matches[index],
        });
    }
}

/// The facts of every model, and the graph built from them.
#[derive(Default)]
pub(crate) struct ModelStore {
    /// By (model, file the model ran on).
    facts: RwLock<BTreeMap<(String, PathBuf), Vec<ModelFact>>>,
    graph: RwLock<Option<Arc<ModelGraph>>>,
}

impl ModelStore {
    pub fn set(&self, model: &str, file: PathBuf, facts: Vec<ModelFact>) {
        let key = (model.to_string(), file);
        let mut all = self.facts.write();
        if facts.is_empty() {
            all.remove(&key);
        } else {
            all.insert(key, facts);
        }
        *self.graph.write() = None;
    }

    pub fn clear(&self, model: Option<&str>) {
        let mut all = self.facts.write();
        match model {
            Some(model) => all.retain(|(name, _), _| name != model),
            None => all.clear(),
        }
        *self.graph.write() = None;
    }

    pub fn all(&self) -> Vec<ModelFileFacts> {
        self.facts
            .read()
            .iter()
            .map(|((model, file), facts)| ModelFileFacts {
                model: model.clone(),
                file: file.clone(),
                facts: facts.clone(),
            })
            .collect()
    }

    /// The graph, built on first use after a change. None when no model
    /// stated anything.
    pub fn graph(&self, host: &dyn FlowHost) -> Option<Arc<ModelGraph>> {
        if let Some(graph) = self.graph.read().as_ref() {
            return Some(Arc::clone(graph));
        }
        let facts = self.facts.read().clone();
        if facts.is_empty() {
            return None;
        }
        let graph = Arc::new(build(host, &facts));
        *self.graph.write() = Some(Arc::clone(&graph));
        Some(graph)
    }
}

impl std::fmt::Debug for ModelStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelStore")
            .field("files", &self.facts.read().len())
            .finish()
    }
}

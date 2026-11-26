//! Value-flow facts for one module: the graph a language's fact builder
//! fills in and the queries in [`crate::query`] walk.

use language_core::{ByteRange, FlowEdgeKind, FlowNodeKind, MutationConfidence, MutationKind};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::HashMap;
use std::sync::Arc;

/// A name in the facts: one shared copy per distinct name in a file.
pub type Name = Arc<str>;

/// A node of the value-flow graph.
#[derive(Debug, Clone)]
pub struct FlowNode {
    pub range: ByteRange,
    pub kind: FlowNodeKind,
    pub name: Option<Name>,
    /// Parameter position.
    pub index: Option<u32>,
    /// Innermost enclosing function (index into [`FileFlow::functions`]).
    pub function: Option<u32>,
    /// True for a binding (as opposed to an expression).
    pub binding: bool,
}

/// A function, arrow function or method in the module.
#[derive(Debug, Clone)]
pub struct FunctionFact {
    pub range: ByteRange,
    /// The function's node in the value-flow graph.
    pub node: u32,
    pub name: Option<Name>,
    /// Where the name is written: the declaration's identifier, the variable
    /// or property it is assigned to, or the method key.
    pub name_range: Option<ByteRange>,
    /// Enclosing function.
    pub parent: Option<u32>,
    /// The class node, for methods.
    pub class_node: Option<u32>,
    /// `get p() {}` or `set p(v) {}`: run by reading or writing the
    /// property, not by calling it.
    pub accessor: Option<Accessor>,
}

/// Which side of a property an accessor function runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accessor {
    Get,
    Set,
}

/// What a variadic parameter collects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variadic {
    /// `...rest`, `*args`: the positional arguments past the others.
    Positional,
    /// `**kwargs`: the keyword arguments no other parameter takes.
    Keywords,
}

/// What a call calls, as far as one module can tell.
#[derive(Debug, Clone)]
pub enum CalleeFact {
    /// The callee expression's node.
    Node(u32),
    /// `object.property(…)`.
    Member { object: u32, property: Name },
}

/// A call, `new` or JSX element.
#[derive(Debug, Clone)]
pub struct CallFact {
    pub range: ByteRange,
    /// Function the call is made from. `None` for module top-level code.
    pub function: Option<u32>,
    pub callee: CalleeFact,
    pub callee_text: Name,
    pub args: Box<[u32]>,
    /// How each argument is passed, by position in `args`, when any is not
    /// plain positional: `Some(name)` for a keyword argument (Python
    /// `f(x=1)`), `Some("*")` / `Some("**")` for unpacking, `Some("...")` for
    /// a spread. Empty when every argument is positional.
    pub arg_names: Box<[Option<Name>]>,
    pub is_new: bool,
    pub is_jsx: bool,
}

/// A place that changes a binding or the value it holds.
#[derive(Debug, Clone)]
pub struct MutationFact {
    /// The binding's node.
    pub binding: u32,
    pub range: ByteRange,
    pub kind: MutationKind,
    pub confidence: MutationConfidence,
    pub name: Option<Name>,
    pub function: Option<u32>,
}

/// An import binding.
#[derive(Debug, Clone)]
pub struct ImportFact {
    pub module: String,
    /// The name asked of the module (`default` for a default import).
    pub name: String,
    pub namespace: bool,
}

/// Value-flow facts for one module.
#[derive(Debug, Default)]
pub struct FileFlow {
    pub nodes: Vec<FlowNode>,
    /// `out_edges[n]`: nodes the value of `n` flows to.
    pub out_edges: Adjacency,
    /// `in_edges[n]`: nodes whose value flows into `n`.
    pub in_edges: Adjacency,
    /// Node ids sorted by range, for [`FileFlow::exact`].
    pub by_span: Vec<u32>,
    pub functions: Vec<FunctionFact>,
    pub fn_by_node: FxHashMap<u32, u32>,
    pub calls: Vec<CallFact>,
    pub call_by_node: FxHashMap<u32, u32>,
    pub mutations: Vec<MutationFact>,
    pub imports: FxHashMap<u32, ImportFact>,
    /// Object literal node -> (property name, value node).
    pub object_members: FxHashMap<u32, Vec<(Name, u32)>>,
    /// Class node -> (method name, function).
    pub class_methods: FxHashMap<u32, Vec<(Name, u32)>>,
    /// Class node -> (field name, the field's node). The initializer flows
    /// into the field's node with a `store` edge.
    pub class_fields: FxHashMap<u32, Vec<(Name, u32)>>,
    /// `require("m")` call node -> "m".
    pub requires: FxHashMap<u32, String>,
    /// CommonJS exports: `module.exports = v` (None) and `exports.f = v`,
    /// `module.exports.f = v` (Some("f")), with the value's node.
    pub commonjs_exports: Vec<(Option<String>, u32)>,
    /// The key of a `property` edge into a destructured binding:
    /// `const { f: g } = o` gives (o, g) -> "f".
    pub property_keys: FxHashMap<(u32, u32), String>,
    /// Class node -> its `extends` expression's node.
    pub class_extends: FxHashMap<u32, u32>,
    /// `super` in `super(…)` and `super.m(…)`.
    pub super_nodes: FxHashSet<u32>,
    /// The position of an `element` edge into a destructured binding:
    /// `const [a, b] = v` gives (v, b) -> 1.
    pub element_index: FxHashMap<(u32, u32), u32>,
    /// Array literal node -> its elements' value nodes by position (up to
    /// the first spread; holes are None).
    pub array_elements: FxHashMap<u32, Vec<Option<u32>>>,
    /// `import("m")` node -> "m".
    pub dynamic_imports: FxHashMap<u32, String>,
    /// Edges that come from a builtin method model (`.then(cb)`,
    /// `.map(cb)`, …) rather than from syntax: they go through a call, so
    /// `throughCalls: false` does not follow them.
    pub model_edges: FxHashSet<(u32, u32)>,
    pub loops: Vec<ByteRange>,
    /// Module-level bindings by name, for languages whose imports name a
    /// module's top-level names directly (Python). Empty for JavaScript,
    /// whose exports the host resolves itself.
    pub module_bindings: FxHashMap<Name, u32>,
    /// A binding, field or function node -> the node of its declared type
    /// (`repo: OrderRepository`, `def f() -> Repo`). Only call resolution
    /// reads it: `repo.find()` looks for `find` on what the type may be.
    pub annotations: FxHashMap<u32, u32>,
    /// A binding, field or function node -> its declared type as written.
    pub declared_types: FxHashMap<u32, Name>,
    /// Function index -> the decorators written on it, as written.
    pub decorators: FxHashMap<u32, Vec<Name>>,
    /// Parameter bindings that collect the rest of the arguments: `...rest`,
    /// `*args` (positional) and `**kwargs` (keywords).
    pub variadic: FxHashMap<u32, Variadic>,
    /// A method's first parameter is the object it is called on (Python's
    /// `self`), so a method call's arguments start at its second parameter.
    pub bound_self: bool,
    /// Syntax the builder saw but does not model, by what it is. Values that
    /// pass through it are not connected to where they go. Used to measure
    /// coverage; queries do not read it.
    pub skipped: HashMap<String, u32>,
    /// Approximate heap bytes held, set when the facts are built.
    pub bytes: usize,
}

impl FileFlow {
    /// The node with exactly this range.
    pub fn exact(&self, range: ByteRange) -> Option<u32> {
        let key = (range.start, range.end);
        self.by_span
            .binary_search_by_key(&key, |id| {
                let range = self.nodes[*id as usize].range;
                (range.start, range.end)
            })
            .ok()
            .map(|position| self.by_span[position])
    }

    /// The node for a syntax range: the exact one, else the tightest node
    /// around it, else the widest node inside it.
    pub fn locate(&self, range: ByteRange) -> Option<u32> {
        if let Some(found) = self.exact(range) {
            return Some(found);
        }
        let around = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| range.is_within(&n.range))
            .min_by_key(|(_, n)| n.range.len());
        if let Some((index, _)) = around {
            return Some(index as u32);
        }
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.range.is_within(&range))
            .max_by_key(|(_, n)| n.range.len())
            .map(|(index, _)| index as u32)
    }

    /// Heap bytes of the range index.
    pub fn span_index_bytes(&self) -> usize {
        self.by_span.capacity() * 4
    }

    /// Approximate heap bytes: the arrays, the maps by capacity, the names.
    pub fn estimate_bytes(&self, names: &FxHashSet<Name>) -> usize {
        use std::mem::size_of;
        let map = |capacity: usize, entry: usize| capacity * (entry + 1);
        let lists = |lists: &FxHashMap<u32, Vec<(Name, u32)>>| {
            map(lists.capacity(), 32)
                + lists
                    .values()
                    .map(|list| list.capacity() * size_of::<(Name, u32)>())
                    .sum::<usize>()
        };
        self.nodes.capacity() * size_of::<FlowNode>()
            + self.out_edges.bytes()
            + self.in_edges.bytes()
            + self.by_span.capacity() * 4
            + self.functions.capacity() * size_of::<FunctionFact>()
            + self.calls.capacity() * size_of::<CallFact>()
            + self.calls.iter().map(|c| c.args.len() * 4).sum::<usize>()
            + self.mutations.capacity() * size_of::<MutationFact>()
            + map(self.fn_by_node.capacity(), 8)
            + map(self.call_by_node.capacity(), 8)
            + map(self.imports.capacity(), 4 + size_of::<ImportFact>())
            + lists(&self.object_members)
            + lists(&self.class_methods)
            + lists(&self.class_fields)
            + map(self.model_edges.capacity(), 8)
            + names.iter().map(|name| name.len() + 16).sum::<usize>()
    }

    /// Sort the node ids by range, for [`FileFlow::exact`]. Call once all
    /// nodes are added.
    pub fn index_spans(&mut self) {
        let mut by_span: Vec<u32> = (0..self.nodes.len() as u32).collect();
        by_span.sort_unstable_by_key(|id| {
            let range = self.nodes[*id as usize].range;
            (range.start, range.end)
        });
        self.by_span = by_span;
    }

    /// The innermost function around a range.
    pub fn function_at(&self, range: ByteRange) -> Option<u32> {
        self.functions
            .iter()
            .enumerate()
            .filter(|(_, f)| range.is_within(&f.range))
            .min_by_key(|(_, f)| f.range.len())
            .map(|(index, _)| index as u32)
    }

    /// The class a function is a method of, looking through nested functions
    /// (an arrow function inside a method still sees the method's `this`).
    pub fn class_of(&self, function: Option<u32>) -> Option<u32> {
        let mut current = function;
        while let Some(index) = current {
            let fact = &self.functions[index as usize];
            if fact.class_node.is_some() {
                return fact.class_node;
            }
            current = fact.parent;
        }
        None
    }
}

/// Edges by node, in one array: node `n`'s are
/// `edges[offsets[n]..offsets[n + 1]]`, in the order they were added.
#[derive(Debug, Default, Clone)]
pub struct Adjacency {
    offsets: Vec<u32>,
    edges: Vec<(u32, FlowEdgeKind)>,
}

impl Adjacency {
    pub fn from_lists(lists: Vec<Vec<(u32, FlowEdgeKind)>>) -> Self {
        let total = lists.iter().map(Vec::len).sum();
        let mut offsets = Vec::with_capacity(lists.len() + 1);
        let mut edges = Vec::with_capacity(total);
        offsets.push(0);
        for list in lists {
            edges.extend(list);
            offsets.push(edges.len() as u32);
        }
        Adjacency { offsets, edges }
    }

    /// Number of edges.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Heap bytes held.
    pub fn bytes(&self) -> usize {
        self.offsets.capacity() * 4 + self.edges.capacity() * 8
    }
}

impl std::ops::Index<usize> for Adjacency {
    type Output = [(u32, FlowEdgeKind)];

    fn index(&self, node: usize) -> &Self::Output {
        &self.edges[self.offsets[node] as usize..self.offsets[node + 1] as usize]
    }
}

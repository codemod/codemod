//! Analyses as values: `callGraph(lang)`, `dataflow(lang)`, `symbols(lang)`,
//! `types(lang)` and `entryPoints(lang)`, user lookups made with
//! `lookup(edges)`, and `crawl(lookups, start, { stopAt })`.
//!
//! A lookup is a read-only map from a node to the edges that leave it, each
//! `{ node, ...meta }`. Built-in lookups answer `get(node)` when asked, from
//! the step's semantic analysis; `lookup(edges)` stores its edges once, keyed
//! by node. Both match nodes by the syntax node (`node.key()`), not by the JS
//! object, so a node from another query finds its edges.
//!
//! `crawl` walks the lookups from the start nodes in Rust: breadth first,
//! each node once, never past a node in `stopAt`. Its result maps each node
//! reached to the edge that reached it first (with `from`, the node it came
//! from), and each start node to null.
//!
//! `join({ from, to, meta })` makes a lookup from two patterns matched across
//! the workspace, with an edge wherever both capture the same text.
//!
//! Across steps, nodes are their keys. A lookup or a crawl's result returned
//! from a step becomes JSON (`toJSON()`), and `lookup(result)` in a later
//! step makes it a lookup again, its keys resolved against that step's
//! files. Anywhere a node goes in (`get`, `crawl`'s start, `lookup`'s
//! edges), its key goes too.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use rquickjs::{
    atom::PredefinedAtom, class::Trace, prelude::Opt, Array, Class, Ctx, Error, Exception,
    Function, IntoJs, JsLifetime, Object, Promise, Result, Value,
};

use super::sg_node::{
    file_of_value, parse_node_key, resolve_module_from, root_for_path, root_for_text,
    specifier_from, SgNodeRjs, SgRootInner,
};
use super::utils::{convert_matcher, JsMatcherRjs};
use super::{DefaultParseLanguage, ParseSemanticProvider};
use crate::sandbox::engine::execution_engine::{JssgFileChanges, TargetDirectory};

/// The languages an analysis covers: one provider each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    JavaScript,
    Python,
}

impl Family {
    pub(crate) fn from_lang(lang: &str) -> Option<Self> {
        match lang.to_ascii_lowercase().as_str() {
            "javascript" | "typescript" | "tsx" | "jsx" | "js" | "ts" => Some(Self::JavaScript),
            "python" | "py" => Some(Self::Python),
            _ => None,
        }
    }

    fn provider_language(self) -> &'static str {
        match self {
            Self::JavaScript => "typescript",
            Self::Python => "python",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::JavaScript => "JavaScript/TypeScript",
            Self::Python => "Python",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Builtin {
    Callers,
    Callees,
    Sources,
    Uses,
    Definition,
    References,
    Imports,
    Importers,
    TypeOf,
}

impl Builtin {
    fn name(self) -> &'static str {
        match self {
            Self::Callers => "callers",
            Self::Callees => "callees",
            Self::Sources => "sources",
            Self::Uses => "uses",
            Self::Definition => "definition",
            Self::References => "references",
            Self::Imports => "imports",
            Self::Importers => "importers",
            Self::TypeOf => "typeOf",
        }
    }
}

/// Whether a `sources` / `uses` edge crosses a call: into a callee (an
/// argument to its parameter, a call to its returned values), or out of a
/// function to its callers (a returned value to the call sites, a parameter
/// to the callers' arguments).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Crossing {
    Into,
    Out,
}

/// Whether `dataflow(lang)` follows values across calls when no
/// `interprocedural` option is given.
const ACROSS_CALLS_BY_DEFAULT: bool = false;

/// Edges stored for one node: the node and its edge objects.
#[derive(Trace, Clone)]
struct Stored<'js> {
    node: Value<'js>,
    value: Value<'js>,
}

/// A read-only map keyed by syntax node: a built-in lookup, a user lookup,
/// or what `crawl` reached.
#[derive(Trace, Clone)]
#[rquickjs::class]
pub struct Lookup<'js> {
    #[qjs(skip_trace)]
    builtin: Option<(Builtin, Family)>,
    /// `sources` and `uses`: follow values across calls (with function
    /// summaries), as `dataflow(lang, { interprocedural })` asked.
    #[qjs(skip_trace)]
    across_calls: bool,
    /// What the entries hold: edges, a crawl's result, or entry points'
    /// reasons. Only edges can be crawled.
    #[qjs(skip_trace)]
    shape: Shape,
    entries: Vec<Stored<'js>>,
    #[qjs(skip_trace)]
    index: HashMap<String, usize>,
}

/// What a stored lookup's entries hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// `node -> [{ node, ...meta }]`: a lookup.
    Edges,
    /// `node -> edge | null`: what `crawl` reached, and how.
    Reached,
    /// `function -> [{ kind, label }]`: `entryPoints(lang)`, why each
    /// function is called from outside.
    Reasons,
}

unsafe impl<'js> JsLifetime<'js> for Lookup<'js> {
    type Changed<'to> = Lookup<'to>;
}

impl<'js> Lookup<'js> {
    fn builtin(kind: Builtin, family: Family) -> Self {
        Self {
            builtin: Some((kind, family)),
            across_calls: false,
            shape: Shape::Edges,
            entries: Vec::new(),
            index: HashMap::new(),
        }
    }

    fn stored(entries: Vec<Stored<'js>>, index: HashMap<String, usize>, shape: Shape) -> Self {
        Self {
            builtin: None,
            across_calls: false,
            shape,
            entries,
            index,
        }
    }

    fn what(&self) -> &'static str {
        match self.builtin {
            Some((kind, _)) => kind.name(),
            None => match self.shape {
                Shape::Reached => "crawl()'s result",
                Shape::Reasons => "entryPoints()",
                Shape::Edges => "lookup()",
            },
        }
    }

    fn not_listable(&self, ctx: &Ctx<'js>, method: &str) -> rquickjs::Error {
        Exception::throw_type(
            ctx,
            &format!(
                "{}.{method}: a built-in lookup is answered node by node and can't be listed. \
                 Ask get(node), or crawl() from start nodes.",
                self.what()
            ),
        )
    }

    /// The edges leaving `node`: an array (or the type, for `typeOf`), or
    /// undefined when there are none.
    fn edges(&self, ctx: &Ctx<'js>, node: &Value<'js>) -> Result<Value<'js>> {
        Ok(self.edges_crossing(ctx, node)?.0)
    }

    /// [`Self::edges`], and for each edge whether it steps into a callee or
    /// out to a function's callers (`sources` and `uses` across calls), for
    /// `crawl` to match calls with. Empty when no edge crosses a call.
    fn edges_crossing(
        &self,
        ctx: &Ctx<'js>,
        node: &Value<'js>,
    ) -> Result<(Value<'js>, Vec<Option<Crossing>>)> {
        let mut crossings = Vec::new();
        let Some((kind, family)) = self.builtin else {
            let key = key_of(ctx, node, self.what())?;
            return Ok((
                match self.index.get(&key) {
                    Some(&i) => self.entries[i].value.clone(),
                    None => Value::new_undefined(ctx.clone()),
                },
                crossings,
            ));
        };
        let class = as_node(ctx, node, kind.name())?;
        let sg = class.borrow();
        let node_family = Family::from_lang(&sg.language_name());
        if node_family != Some(family) {
            return Err(Exception::throw_type(
                ctx,
                &format!(
                    "{}.get(node): this lookup is for {}, and the node is {}.",
                    kind.name(),
                    family.name(),
                    sg.language_name()
                ),
            ));
        }
        let edges = Array::new(ctx.clone())?;
        let push = |edge: Object<'js>| -> Result<()> { edges.set(edges.len(), edge) };
        match kind {
            Builtin::Callers | Builtin::Callees => {
                let found = if kind == Builtin::Callers {
                    renamed(ctx, sg.callers(ctx.clone(), Opt(None)), "callers()", kind)?
                } else {
                    renamed(ctx, sg.callees(ctx.clone(), Opt(None)), "callees()", kind)?
                };
                for entry in array_items(&found)? {
                    let entry: Object = entry.get()?;
                    if kind == Builtin::Callees
                        && entry.get::<_, Option<bool>>("resolved")? == Some(false)
                    {
                        continue;
                    }
                    let edge = Object::new(ctx.clone())?;
                    edge.set("node", entry.get::<_, Value>("node")?)?;
                    let site = entry
                        .get::<_, Option<Object>>("callSite")?
                        .map(|site| site.get::<_, Value>("node"))
                        .transpose()?;
                    edge.set("site", site)?;
                    edge.set("via", entry.get::<_, Value>("via")?)?;
                    push(edge)?;
                }
            }
            Builtin::Sources | Builtin::Uses => {
                // One hop, answered directly; across calls when on.
                let options = Object::new(ctx.clone())?;
                options.set("oneHop", true)?;
                options.set("acrossCalls", self.across_calls)?;
                let options = Opt(Some(options.into_value()));
                let found = if kind == Builtin::Sources {
                    renamed(
                        ctx,
                        sg.flows_from(ctx.clone(), options),
                        "flowsFrom()",
                        kind,
                    )?
                } else {
                    renamed(ctx, sg.flows_to(ctx.clone(), options), "flowsTo()", kind)?
                };
                for step in array_items(&found)? {
                    let step: Object = step.get()?;
                    let edge = Object::new(ctx.clone())?;
                    for field in ["node", "kind", "via", "name", "index", "boundary"] {
                        edge.set(field, step.get::<_, Value>(field)?)?;
                    }
                    if step.get::<_, Option<bool>>("stop")? == Some(true) {
                        edge.set("stop", true)?;
                    }
                    crossings.push(
                        match step.get::<_, Option<String>>("crossing")?.as_deref() {
                            Some("into") => Some(Crossing::Into),
                            Some("out") => Some(Crossing::Out),
                            _ => None,
                        },
                    );
                    push(edge)?;
                }
            }
            Builtin::Definition => {
                // Through imports, to the declaration: a lookup's edges land
                // on what the name means, so a crawl can go on from there.
                let options = Object::new(ctx.clone())?;
                options.set("follow", true)?;
                let found = renamed(
                    ctx,
                    sg.definition(ctx.clone(), Opt(Some(options))),
                    "definition()",
                    kind,
                )?;
                if let Some(definition) = found.as_object() {
                    let edge = Object::new(ctx.clone())?;
                    for field in ["node", "nameNode", "kind", "exportName"] {
                        edge.set(field, definition.get::<_, Value>(field)?)?;
                    }
                    push(edge)?;
                }
            }
            Builtin::References => {
                let found = renamed(ctx, sg.references(ctx.clone()), "references()", kind)?;
                for file in array_items(&found)? {
                    let file: Object = file.get()?;
                    let nodes: Value = file.get("nodes")?;
                    for node in array_items(&nodes)? {
                        let edge = Object::new(ctx.clone())?;
                        edge.set("node", node)?;
                        push(edge)?;
                    }
                }
            }
            Builtin::Imports | Builtin::Importers => {
                for edge in module_edges(ctx, &sg, kind)? {
                    push(edge)?;
                }
            }
            Builtin::TypeOf => {
                let found = renamed(ctx, sg.type_info(ctx.clone()), "typeInfo()", kind)?;
                return Ok((
                    if found.is_null() {
                        Value::new_undefined(ctx.clone())
                    } else {
                        found
                    },
                    crossings,
                ));
            }
        }
        if edges.is_empty() {
            return Ok((Value::new_undefined(ctx.clone()), crossings));
        }
        Ok((edges.into_js(ctx)?, crossings))
    }

    fn list(&self, ctx: &Ctx<'js>, method: &str) -> Result<&[Stored<'js>]> {
        if self.builtin.is_some() {
            return Err(self.not_listable(ctx, method));
        }
        Ok(&self.entries)
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl<'js> Lookup<'js> {
    /// The edges leaving `node` (`{ node, ...meta }`), or undefined.
    pub fn get(&self, ctx: Ctx<'js>, node: Value<'js>) -> Result<Value<'js>> {
        self.edges(&ctx, &node)
    }

    pub fn has(&self, ctx: Ctx<'js>, node: Value<'js>) -> Result<bool> {
        Ok(!self.edges(&ctx, &node)?.is_undefined())
    }

    #[qjs(get)]
    pub fn size(&self, ctx: Ctx<'js>) -> Result<usize> {
        Ok(self.list(&ctx, "size")?.len())
    }

    pub fn keys(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let items: Vec<Value> = self
            .list(&ctx, "keys()")?
            .iter()
            .map(|e| e.node.clone())
            .collect();
        iterator_over(&ctx, items)
    }

    pub fn values(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let items: Vec<Value> = self
            .list(&ctx, "values()")?
            .iter()
            .map(|e| e.value.clone())
            .collect();
        iterator_over(&ctx, items)
    }

    pub fn entries(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        let mut items = Vec::new();
        for entry in self.list(&ctx, "entries()")? {
            let pair = Array::new(ctx.clone())?;
            pair.set(0, entry.node.clone())?;
            pair.set(1, entry.value.clone())?;
            items.push(pair.into_value());
        }
        iterator_over(&ctx, items)
    }

    #[qjs(rename = PredefinedAtom::SymbolIterator)]
    pub fn iterate(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        self.entries(ctx)
    }

    #[qjs(rename = "forEach")]
    pub fn for_each(
        &self,
        ctx: Ctx<'js>,
        callback: Function<'js>,
        this: Opt<Value<'js>>,
    ) -> Result<()> {
        let entries = self.list(&ctx, "forEach()")?.to_vec();
        let map: Value = Class::instance(ctx.clone(), self.clone())?.into_value();
        let this = this.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
        for entry in entries {
            callback.call::<_, ()>((
                rquickjs::function::This(this.clone()),
                entry.value,
                entry.node,
                map.clone(),
            ))?;
        }
        Ok(())
    }

    #[qjs(get, rename = PredefinedAtom::SymbolToStringTag)]
    pub fn to_string_tag(&self) -> &'static str {
        "Lookup"
    }

    /// The lookup as JSON, so a step can return it: nodes become their keys.
    /// `{ lookup: [[from, to, meta], ...], nodeFields }` for edges, or
    /// `{ reached: [[node, edge | null], ...], nodeFields }` for a crawl's
    /// result, `{ entryPoints: [[function, reasons], ...] }` for entry points, where `nodeFields` names the edge fields that held nodes.
    /// `lookup(json)` in a later step makes it a lookup again.
    #[qjs(rename = "toJSON")]
    pub fn to_json(&self, ctx: Ctx<'js>) -> Result<Object<'js>> {
        if let Some((kind, _)) = self.builtin {
            return Err(Exception::throw_type(
                &ctx,
                &format!(
                    "{}: a built-in lookup can't be a step's result; it answers node by node \
                     in the step that asks. Ask for it again in the later step.",
                    kind.name()
                ),
            ));
        }
        let mut node_fields: BTreeSet<String> = BTreeSet::new();
        let list = Array::new(ctx.clone())?;
        for entry in &self.entries {
            let from = key_of(&ctx, &entry.node, "toJSON()")?;
            if self.shape == Shape::Reasons {
                let row = Array::new(ctx.clone())?;
                row.set(0, from)?;
                row.set(1, entry.value.clone())?;
                list.set(list.len(), row)?;
                continue;
            }
            if self.shape == Shape::Reached {
                let row = Array::new(ctx.clone())?;
                row.set(0, from)?;
                match entry.value.as_object() {
                    Some(edge) => row.set(1, plain_edge(&ctx, edge, None, &mut node_fields)?)?,
                    None => row.set(1, Value::new_null(ctx.clone()))?,
                }
                list.set(list.len(), row)?;
                continue;
            }
            for edge in array_items(&entry.value)? {
                let edge: Object = edge.get()?;
                let to = key_of(&ctx, &edge.get::<_, Value>("node")?, "toJSON()")?;
                let row = Array::new(ctx.clone())?;
                row.set(0, from.clone())?;
                row.set(1, to)?;
                row.set(2, plain_edge(&ctx, &edge, Some("node"), &mut node_fields)?)?;
                list.set(list.len(), row)?;
            }
        }
        let json = Object::new(ctx.clone())?;
        let name = match self.shape {
            Shape::Edges => "lookup",
            Shape::Reached => "reached",
            Shape::Reasons => "entryPoints",
        };
        json.set(name, list)?;
        json.set("nodeFields", node_fields.into_iter().collect::<Vec<_>>())?;
        Ok(json)
    }
}

/// An edge's fields with nodes as their keys, leaving out `skip`; records
/// which fields held nodes.
fn plain_edge<'js>(
    ctx: &Ctx<'js>,
    edge: &Object<'js>,
    skip: Option<&str>,
    node_fields: &mut BTreeSet<String>,
) -> Result<Object<'js>> {
    let plain = Object::new(ctx.clone())?;
    for prop in edge.props::<String, Value>() {
        let (name, value) = prop?;
        if Some(name.as_str()) == skip {
            continue;
        }
        match Class::<SgNodeRjs>::from_value(&value) {
            Ok(node) => {
                plain.set(name.as_str(), node.borrow().key())?;
                node_fields.insert(name);
            }
            Err(_) => plain.set(name, value)?,
        }
    }
    Ok(plain)
}

/// Freeze each stored edge list, so a lookup reads the same in every step.
fn freeze_all<'js>(ctx: &Ctx<'js>, entries: &[Stored<'js>]) -> Result<()> {
    let object: Object = ctx.globals().get("Object")?;
    let freeze: Function = object.get("freeze")?;
    for entry in entries {
        freeze.call::<_, Value>((entry.value.clone(),))?;
    }
    Ok(())
}

fn array_items<'js>(value: &Value<'js>) -> Result<Vec<Value<'js>>> {
    match value.as_array() {
        Some(array) => array.iter().collect(),
        None => Ok(Vec::new()),
    }
}

/// An iterator over `items`, from an array's `values()`.
fn iterator_over<'js>(ctx: &Ctx<'js>, items: Vec<Value<'js>>) -> Result<Value<'js>> {
    let array = Array::new(ctx.clone())?;
    for (i, item) in items.into_iter().enumerate() {
        array.set(i, item)?;
    }
    let values: Function = array.as_object().get("values")?;
    values.call((rquickjs::function::This(array),))
}

/// A node's key: from the node, or a key itself (`node.key()`, or a node an
/// earlier step returned).
fn key_of<'js>(ctx: &Ctx<'js>, node: &Value<'js>, what: &str) -> Result<String> {
    if let Some(key) = node.as_string() {
        let key = key.to_string()?;
        if parse_node_key(&key).is_none() {
            return Err(not_a_key(ctx, &key, what));
        }
        return Ok(key);
    }
    let class = node_from_value(ctx, node, what)?;
    let key = class.borrow().key();
    Ok(key)
}

/// A node, or the node a key names in this step's files.
fn as_node<'js>(
    ctx: &Ctx<'js>,
    value: &Value<'js>,
    what: &str,
) -> Result<Class<'js, SgNodeRjs<'js>>> {
    match value.as_string() {
        Some(key) => {
            let node = Nodes::default().resolve(ctx, &key.to_string()?, what)?;
            Class::instance(ctx.clone(), node)
        }
        None => node_from_value(ctx, value, what),
    }
}

fn not_a_key(ctx: &Ctx<'_>, key: &str, what: &str) -> rquickjs::Error {
    Exception::throw_type(
        ctx,
        &format!("{what}: \"{key}\" is not a node or a node.key() (file:start-end:kind)"),
    )
}

/// A file's text as the step started: in a dry run, as earlier steps left
/// it in memory.
fn step_text(ctx: &Ctx<'_>, path: &std::path::Path) -> Option<String> {
    let pending = ctx
        .userdata::<JssgFileChanges>()
        .and_then(|changes| changes.pending.clone());
    match pending {
        Some(pending) => pending.text_at_step_start(path).ok(),
        None => std::fs::read_to_string(path).ok(),
    }
}

/// Turns keys back into nodes, parsing each file once.
#[derive(Default)]
struct Nodes {
    roots: HashMap<String, Option<Arc<SgRootInner>>>,
}

impl Nodes {
    fn resolve<'js>(&mut self, ctx: &Ctx<'js>, key: &str, what: &str) -> Result<SgNodeRjs<'js>> {
        let Some((file, start, end, kind)) = parse_node_key(key) else {
            return Err(not_a_key(ctx, key, what));
        };
        let root = self
            .roots
            .entry(file.to_string())
            .or_insert_with(|| {
                let path = std::path::Path::new(file);
                let text = step_text(ctx, path)?;
                root_for_text(
                    step_provider(ctx),
                    path,
                    text,
                    target_directory(ctx).as_deref(),
                )
            })
            .clone();
        let gone = |why: &str| {
            Exception::throw_type(
                ctx,
                &format!("{what}: no node for \"{key}\" in this step's files: {why}"),
            )
        };
        let root = root.ok_or_else(|| gone("the file can't be read"))?;
        SgNodeRjs::node_with(&root, start, end, kind)
            .ok_or_else(|| gone("the file changed after the step that made the key"))
    }

    fn value<'js>(&mut self, ctx: &Ctx<'js>, key: &str, what: &str) -> Result<Value<'js>> {
        self.resolve(ctx, key, what)?.into_js(ctx)
    }

    /// A node as given, or the node its key names.
    fn node<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        value: Value<'js>,
        what: &str,
    ) -> Result<Option<Value<'js>>> {
        if let Some(key) = value.as_string() {
            return self.value(ctx, &key.to_string()?, what).map(Some);
        }
        Ok(Class::<SgNodeRjs>::from_value(&value).ok().map(|_| value))
    }
}

/// The items of an iterable (an array, a Set, a generator).
fn iterate_values<'js>(
    ctx: &Ctx<'js>,
    iterable: &Value<'js>,
    what: &str,
) -> Result<Vec<Value<'js>>> {
    if let Some(array) = iterable.as_array() {
        return array.iter().collect();
    }
    let not_iterable = || Exception::throw_type(ctx, &format!("{what} must be iterable"));
    let object = iterable.as_object().ok_or_else(not_iterable)?;
    let make: Option<Function> = object.get(PredefinedAtom::SymbolIterator)?;
    let make = make.ok_or_else(not_iterable)?;
    let iterator: Object = make.call((rquickjs::function::This(object.clone()),))?;
    let next: Function = iterator.get(PredefinedAtom::Next)?;
    let mut items = Vec::new();
    loop {
        let step: Object = next.call((rquickjs::function::This(iterator.clone()),))?;
        if step
            .get::<_, Option<bool>>(PredefinedAtom::Done)?
            .unwrap_or(false)
        {
            break;
        }
        items.push(step.get(PredefinedAtom::Value)?);
    }
    Ok(items)
}

fn family_of(ctx: &Ctx<'_>, method: &str, lang: &str) -> Result<Family> {
    Family::from_lang(lang).ok_or_else(|| {
        Exception::throw_type(
            ctx,
            &format!(
                "{method}(\"{lang}\"): semantic analysis covers \"typescript\", \"tsx\", \
                 \"javascript\" and \"python\"."
            ),
        )
    })
}

fn pair<'js>(ctx: &Ctx<'js>, family: Family, fields: &[(&str, Builtin)]) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    for (name, kind) in fields {
        object.set(
            *name,
            Class::instance(ctx.clone(), Lookup::builtin(*kind, family))?,
        )?;
    }
    Ok(object)
}

/// `callGraph(lang)`: `{ callers, callees }`.
pub(crate) fn call_graph_rjs<'js>(ctx: Ctx<'js>, lang: String) -> Result<Object<'js>> {
    let family = family_of(&ctx, "callGraph", &lang)?;
    pair(
        &ctx,
        family,
        &[("callers", Builtin::Callers), ("callees", Builtin::Callees)],
    )
}

/// `dataflow(lang, { interprocedural })`: `{ sources, uses }`. With
/// `interprocedural`, values are followed across calls, using each called
/// function's summary of what it returns and stores.
pub(crate) fn dataflow_rjs<'js>(
    ctx: Ctx<'js>,
    lang: String,
    options: Opt<Value<'js>>,
) -> Result<Object<'js>> {
    let family = family_of(&ctx, "dataflow", &lang)?;
    let mut across_calls = ACROSS_CALLS_BY_DEFAULT;
    match options.0 {
        None => {}
        Some(value) if value.is_undefined() || value.is_null() => {}
        Some(value) => {
            let Some(object) = value.into_object() else {
                return Err(Exception::throw_type(
                    &ctx,
                    "dataflow(lang, options): options must be an object",
                ));
            };
            for key in object.keys::<String>() {
                let key = key?;
                if key != "interprocedural" {
                    return Err(Exception::throw_type(
                        &ctx,
                        &format!(
                            "dataflow(lang, options): unknown option `{key}`; expected `interprocedural`"
                        ),
                    ));
                }
            }
            if let Some(on) = object.get::<_, Option<bool>>("interprocedural")? {
                across_calls = on;
            }
        }
    }
    let object = Object::new(ctx.clone())?;
    for (name, kind) in [("sources", Builtin::Sources), ("uses", Builtin::Uses)] {
        let mut lookup = Lookup::builtin(kind, family);
        lookup.across_calls = across_calls;
        object.set(name, Class::instance(ctx.clone(), lookup)?)?;
    }
    Ok(object)
}

/// `symbols(lang)`: the `definition`, `references`, `imports` and
/// `importers` lookups, and two functions for codemods that rewrite imports:
/// `resolveModule(file, specifier)` and `specifierFor(file, target, options)`.
pub(crate) fn symbols_rjs<'js>(ctx: Ctx<'js>, lang: String) -> Result<Object<'js>> {
    let family = family_of(&ctx, "symbols", &lang)?;
    let object = pair(
        &ctx,
        family,
        &[
            ("definition", Builtin::Definition),
            ("references", Builtin::References),
            ("imports", Builtin::Imports),
            ("importers", Builtin::Importers),
        ],
    )?;
    object.set(
        "resolveModule",
        Function::new(ctx.clone(), resolve_module_rjs)?.with_name("resolveModule")?,
    )?;
    object.set(
        "specifierFor",
        Function::new(ctx.clone(), specifier_for_rjs)?.with_name("specifierFor")?,
    )?;
    Ok(object)
}

/// The step's provider and `file`'s path, or why a module query can't run.
fn module_query<'js>(
    ctx: &Ctx<'js>,
    file: &Value<'js>,
    method: &str,
) -> Result<(Arc<dyn language_core::SemanticProvider>, std::path::PathBuf)> {
    let provider = step_provider(ctx).ok_or_else(|| {
        Exception::throw_message(
            ctx,
            &format!(
                "{method}(): there is no semantic analysis here. It works in workflow steps \
                 (and with `codemod jssg run --semantic-workspace <dir>`)."
            ),
        )
    })?;
    let target = target_directory(ctx);
    let path = file_of_value(ctx, file, target.as_deref(), &format!("{method}(file)"))?;
    Ok((provider, path))
}

/// `symbols(lang).resolveModule(file, specifier)`.
fn resolve_module_rjs<'js>(
    ctx: Ctx<'js>,
    file: Value<'js>,
    specifier: String,
) -> Result<Value<'js>> {
    let (provider, path) = module_query(&ctx, &file, "resolveModule")?;
    resolve_module_from(&ctx, &*provider, &path, &specifier)
}

/// `symbols(lang).specifierFor(file, target, options)`.
fn specifier_for_rjs<'js>(
    ctx: Ctx<'js>,
    file: Value<'js>,
    target: Value<'js>,
    options: Opt<Value<'js>>,
) -> Result<Value<'js>> {
    let (provider, path) = module_query(&ctx, &file, "specifierFor")?;
    let dir = target_directory(&ctx);
    let target = file_of_value(&ctx, &target, dir.as_deref(), "specifierFor(target)")?;
    specifier_from(&ctx, &*provider, &path, &target, options)
}

/// `imports` and `importers` edges for a file's root node: each to the root
/// node of the other file, with the statement or call that names the module
/// (`site`), its `kind` and its `specifier`.
fn module_edges<'js>(
    ctx: &Ctx<'js>,
    sg: &SgNodeRjs<'js>,
    kind: Builtin,
) -> Result<Vec<Object<'js>>> {
    let what = kind.name();
    if !sg.is_root() {
        return Err(Exception::throw_type(
            ctx,
            &format!(
                "{what}.get(node): modules are files, so give a file's root node \
                 (parse(file).root()), not a {}.",
                sg.inner_node.get_node().kind()
            ),
        ));
    }
    let file = std::path::PathBuf::from(sg.root.canonical_filename());
    if file.as_os_str().is_empty() {
        return Err(Exception::throw_message(
            ctx,
            &format!("{what}.get(node): this root was parsed from a string, not a file."),
        ));
    }
    let provider = sg
        .root
        .semantic_provider
        .clone()
        .or_else(|| step_provider(ctx))
        .ok_or_else(|| {
            Exception::throw_message(
                ctx,
                &format!("{what}.get(node): there is no semantic analysis here."),
            )
        })?;
    let failed = |e: language_core::SemanticError| {
        Exception::throw_message(ctx, &format!("{what}.get(node): {e}"))
    };
    let mut roots: HashMap<std::path::PathBuf, Option<Arc<SgRootInner>>> = HashMap::new();
    roots.insert(file.clone(), Some(Arc::clone(&sg.root)));
    let mut root_of = |path: &std::path::Path| -> Option<Arc<SgRootInner>> {
        roots
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                root_for_text(
                    Some(Arc::clone(&provider)),
                    path,
                    step_text(ctx, path)?,
                    target_directory(ctx).as_deref(),
                )
            })
            .clone()
    };
    // (file the module is named in, the other end, range, kind, specifier)
    let mut found = Vec::new();
    if kind == Builtin::Imports {
        for import in provider.imports(&file).map_err(failed)? {
            found.push((
                file.clone(),
                import.target,
                import.range,
                import.kind,
                import.specifier,
            ));
        }
    } else {
        for importing in provider.importers(&file).map_err(failed)?.files {
            for importer in importing.importers {
                found.push((
                    importing.file_path.clone(),
                    importing.file_path.clone(),
                    importer.range,
                    importer.kind,
                    importer.specifier,
                ));
            }
        }
    }
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for (site_file, other, range, import_kind, specifier) in found {
        let (Some(site_root), Some(other_root)) = (root_of(&site_file), root_of(&other)) else {
            continue;
        };
        let Some(mut site) = SgNodeRjs::node_at(&site_root, range) else {
            continue;
        };
        // A static import is found by its binding: the edge names the statement.
        if import_kind == language_core::ImportKind::Import {
            if let Some(statement) = site.ancestor_of_kind("import_statement") {
                site = statement;
            }
        }
        let node = SgNodeRjs::root_node(&other_root);
        if !seen.insert((node.key(), site.key())) {
            continue;
        }
        let edge = Object::new(ctx.clone())?;
        edge.set("node", node)?;
        edge.set("site", site)?;
        edge.set("kind", import_kind.as_str())?;
        edge.set("specifier", specifier)?;
        edges.push(edge);
    }
    Ok(edges)
}

/// `types(lang)`: `{ typeOf }`. A step that imports `types` gets them: the
/// TypeScript 7 checker for JavaScript and TypeScript, ty for Python.
pub(crate) fn types_rjs<'js>(ctx: Ctx<'js>, lang: String) -> Result<Object<'js>> {
    let family = family_of(&ctx, "types", &lang)?;
    pair(&ctx, family, &[("typeOf", Builtin::TypeOf)])
}

/// `entryPoints(lang)`: every function in the workspace something outside
/// its code calls, mapped to why: `[{ kind, label }]`, where `kind` is
/// `"export"` for the package's public API, else the declaring rule's kind.
pub(crate) fn entry_points_rjs<'js>(ctx: Ctx<'js>, lang: String) -> Result<Lookup<'js>> {
    let family = family_of(&ctx, "entryPoints", &lang)?;
    let provider = step_provider(&ctx).ok_or_else(|| {
        Exception::throw_message(
            &ctx,
            "entryPoints(): there is no semantic analysis here. It works inside workflow steps.",
        )
    })?;
    let found = provider
        .list_entry_points(family.provider_language())
        .map_err(|e| Exception::throw_message(&ctx, &format!("entryPoints(): {e}")))?;
    let target = target_directory(&ctx);
    let mut entries: Vec<Stored<'js>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut roots: Vec<(std::path::PathBuf, Arc<SgRootInner>)> = Vec::new();
    for function in found {
        let root = match roots.iter().find(|(path, _)| *path == function.file_path) {
            Some((_, root)) => Arc::clone(root),
            None => {
                let Some(root) = root_for_path(
                    Some(Arc::clone(&provider)),
                    &function.file_path,
                    target.as_deref(),
                ) else {
                    continue;
                };
                roots.push((function.file_path.clone(), Arc::clone(&root)));
                root
            }
        };
        let Some(node) = SgNodeRjs::node_at(&root, function.range) else {
            continue;
        };
        let key = node.key();
        if index.contains_key(&key) {
            continue;
        }
        let reasons = Array::new(ctx.clone())?;
        for entry in &function.entry_points {
            let reason = Object::new(ctx.clone())?;
            reason.set("kind", entry.kind.clone())?;
            reason.set("label", entry.label.clone())?;
            reasons.set(reasons.len(), reason)?;
        }
        index.insert(key, entries.len());
        entries.push(Stored {
            node: Class::instance(ctx.clone(), node)?.into_value(),
            value: reasons.into_value(),
        });
    }
    freeze_all(&ctx, &entries)?;
    Ok(Lookup::stored(entries, index, Shape::Reasons))
}

fn target_directory(ctx: &Ctx<'_>) -> Option<std::path::PathBuf> {
    ctx.userdata::<TargetDirectory>().map(|t| t.0.clone())
}

fn step_provider(ctx: &Ctx<'_>) -> Option<Arc<dyn language_core::SemanticProvider>> {
    ctx.userdata::<ParseSemanticProvider>()
        .map(|p| Arc::clone(&p.0))
}

fn node_from_value<'js>(
    ctx: &Ctx<'js>,
    value: &Value<'js>,
    what: &str,
) -> Result<Class<'js, SgNodeRjs<'js>>> {
    Class::<SgNodeRjs>::from_value(value).map_err(|_| {
        Exception::throw_type(
            ctx,
            &format!("{what}: expected an SgNode. For a flow step or an edge, pass its `.node`."),
        )
    })
}

/// `lookup(edges)`: a lookup from `[from, to, meta?]` edges, where `from`
/// and `to` are nodes or their keys; or `lookup(json)`, a lookup or a
/// crawl's result an earlier step returned, made a lookup again.
pub(crate) fn lookup_rjs<'js>(ctx: Ctx<'js>, edges: Value<'js>) -> Result<Lookup<'js>> {
    let mut nodes = Nodes::default();
    if let Some(json) = edges.as_object().filter(|_| edges.as_array().is_none()) {
        let node_fields: Option<Vec<String>> = json.get("nodeFields")?;
        let node_fields = node_fields.unwrap_or_default();
        let reached: Value = json.get("reached")?;
        if !reached.is_undefined() {
            return reached_from_json(&ctx, &mut nodes, &reached, &node_fields);
        }
        let reasons: Value = json.get("entryPoints")?;
        if !reasons.is_undefined() {
            return reasons_from_json(&ctx, &mut nodes, &reasons);
        }
        let list: Value = json.get("lookup")?;
        if !list.is_undefined() {
            return edges_lookup(&ctx, &mut nodes, &list, &node_fields);
        }
    }
    edges_lookup(&ctx, &mut nodes, &edges, &[])
}

/// A lookup answers through the node methods, whose errors name the method
/// (`flowsTo() on ...`). Name the lookup instead (`uses.get(node) on ...`),
/// since that is what the codemod called; most of those methods are not
/// visible to JavaScript.
fn renamed<'js, T>(ctx: &Ctx<'js>, result: Result<T>, method: &str, kind: Builtin) -> Result<T> {
    match result {
        Err(Error::Exception) => {
            let thrown = ctx.catch();
            if let Some(error) = thrown.as_object() {
                if let Ok(message) = error.get::<_, String>("message") {
                    let lookup = format!("{}.get(node)", kind.name());
                    let _ = error.set("message", message.replace(method, &lookup));
                }
            }
            Err(ctx.throw(thrown))
        }
        other => other,
    }
}

fn edges_lookup<'js>(
    ctx: &Ctx<'js>,
    nodes: &mut Nodes,
    edges: &Value<'js>,
    node_fields: &[String],
) -> Result<Lookup<'js>> {
    let mut entries: Vec<Stored<'js>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (n, item) in iterate_values(ctx, edges, "lookup(edges)")?
        .into_iter()
        .enumerate()
    {
        let what = format!("lookup(edges): edge {n}");
        let bad = |why: &str| Exception::throw_type(ctx, &format!("{what} {why}"));
        let parts = item
            .as_array()
            .ok_or_else(|| bad("must be [from, to] or [from, to, meta]"))?;
        let meta: Value = parts.get(2)?;
        let from_key = key_of(ctx, &parts.get(0)?, &what)
            .map_err(|_| bad("has a `from` that is not a node or a node's key"))?;
        key_of(ctx, &parts.get(1)?, &what)
            .map_err(|_| bad("has a `to` that is not a node or a node's key"))?;
        let from = nodes.node(ctx, parts.get(0)?, &what)?.expect("checked");
        let to = nodes.node(ctx, parts.get(1)?, &what)?.expect("checked");
        let edge = Object::new(ctx.clone())?;
        edge.set("node", to)?;
        if let Some(meta) = meta.as_object() {
            for prop in meta.props::<String, Value>() {
                let (name, value) = prop?;
                if name == "node" || name == "from" {
                    return Err(bad(&format!(
                        "has `{name}` in its meta, and the edge sets it"
                    )));
                }
                let value = match value.as_string() {
                    Some(key) if node_fields.contains(&name) => {
                        nodes.value(ctx, &key.to_string()?, &what)?
                    }
                    _ => value,
                };
                edge.set(name, value)?;
            }
        } else if !meta.is_undefined() && !meta.is_null() {
            return Err(bad("has a meta that is not an object"));
        }
        let slot = match index.get(&from_key) {
            Some(&i) => i,
            None => {
                index.insert(from_key, entries.len());
                entries.push(Stored {
                    node: from,
                    value: Array::new(ctx.clone())?.into_value(),
                });
                entries.len() - 1
            }
        };
        let list = entries[slot].value.as_array().cloned().expect("an array");
        list.set(list.len(), edge)?;
    }
    freeze_all(ctx, &entries)?;
    Ok(Lookup::stored(entries, index, Shape::Edges))
}

/// A crawl's result an earlier step returned: `[[node, edge | null], ...]`.
fn reached_from_json<'js>(
    ctx: &Ctx<'js>,
    nodes: &mut Nodes,
    rows: &Value<'js>,
    node_fields: &[String],
) -> Result<Lookup<'js>> {
    let mut entries: Vec<Stored<'js>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (n, row) in iterate_values(ctx, rows, "lookup(reached)")?
        .into_iter()
        .enumerate()
    {
        let what = format!("lookup(reached): node {n}");
        let bad = || Exception::throw_type(ctx, &format!("{what} must be [node, edge | null]"));
        let row = row.as_array().ok_or_else(bad)?;
        let key = key_of(ctx, &row.get(0)?, &what)?;
        let node = nodes.node(ctx, row.get(0)?, &what)?.ok_or_else(bad)?;
        let edge: Value = row.get(1)?;
        let value = match edge.as_object() {
            Some(edge) => {
                let reached = Object::new(ctx.clone())?;
                for prop in edge.props::<String, Value>() {
                    let (name, value) = prop?;
                    let value = match value.as_string() {
                        Some(key) if node_fields.contains(&name) => {
                            nodes.value(ctx, &key.to_string()?, &what)?
                        }
                        _ => value,
                    };
                    reached.set(name, value)?;
                }
                reached.set("node", node.clone())?;
                reached.into_value()
            }
            None => Value::new_null(ctx.clone()),
        };
        if index.contains_key(&key) {
            continue;
        }
        index.insert(key, entries.len());
        entries.push(Stored { node, value });
    }
    Ok(Lookup::stored(entries, index, Shape::Reached))
}

/// `lookup(json)` for entry points an earlier step returned:
/// `[[function, reasons], ...]`.
fn reasons_from_json<'js>(
    ctx: &Ctx<'js>,
    nodes: &mut Nodes,
    rows: &Value<'js>,
) -> Result<Lookup<'js>> {
    let mut entries: Vec<Stored<'js>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (n, row) in iterate_values(ctx, rows, "lookup(entryPoints)")?
        .into_iter()
        .enumerate()
    {
        let what = format!("lookup(entryPoints): function {n}");
        let bad = || Exception::throw_type(ctx, &format!("{what} must be [function, reasons]"));
        let row = row.as_array().ok_or_else(bad)?;
        let key = key_of(ctx, &row.get(0)?, &what)?;
        let node = nodes.node(ctx, row.get(0)?, &what)?.ok_or_else(bad)?;
        let value: Value = row.get(1)?;
        if value.as_array().is_none() {
            return Err(bad());
        }
        if index.contains_key(&key) {
            continue;
        }
        index.insert(key, entries.len());
        entries.push(Stored { node, value });
    }
    freeze_all(ctx, &entries)?;
    Ok(Lookup::stored(entries, index, Shape::Reasons))
}

/// `crawl(lookups, start, { stopAt })`: every node reached from `start`
/// through the lookups' edges.
pub(crate) fn crawl_rjs<'js>(
    ctx: Ctx<'js>,
    lookups: Value<'js>,
    start: Value<'js>,
    options: Opt<Value<'js>>,
) -> Result<Promise<'js>> {
    let (promise, resolve, reject) = Promise::new(&ctx)?;
    match crawl(&ctx, lookups, start, options.0) {
        Ok(reached) => resolve.call::<_, ()>((reached,))?,
        Err(rquickjs::Error::Exception) => reject.call::<_, ()>((ctx.catch(),))?,
        Err(other) => return Err(other),
    }
    Ok(promise)
}

fn crawl<'js>(
    ctx: &Ctx<'js>,
    lookups: Value<'js>,
    start: Value<'js>,
    options: Option<Value<'js>>,
) -> Result<Lookup<'js>> {
    let mut walk: Vec<Lookup<'js>> = Vec::new();
    let candidates = if lookups.as_array().is_some() {
        iterate_values(ctx, &lookups, "crawl(lookups)")?
    } else {
        vec![lookups]
    };
    for candidate in candidates {
        let lookup = Class::<Lookup>::from_value(&candidate).map_err(|_| {
            let from_a_step = candidate
                .as_object()
                .is_some_and(|o| o.contains_key("lookup").unwrap_or(false));
            Exception::throw_type(
                ctx,
                if from_a_step {
                    "crawl(lookups): this is a lookup an earlier step returned, as JSON. \
                     Make it a lookup again with lookup(result)."
                } else {
                    "crawl(lookups): each lookup must be a built-in lookup or one lookup(edges) made. \
                     Make a Map into one with lookup([...map].flatMap(([from, tos]) => tos.map((to) => [from, to])))."
                },
            )
        })?;
        let lookup = lookup.borrow().clone();
        match lookup.shape {
            Shape::Edges => {}
            Shape::Reached => {
                return Err(Exception::throw_type(
                    ctx,
                    "crawl(lookups): a crawl's result is not a lookup.",
                ))
            }
            Shape::Reasons => {
                return Err(Exception::throw_type(
                    ctx,
                    "crawl(lookups): entryPoints() maps a function to why it is an entry point, \
                     not to nodes. To crawl from the entry points, pass it as the start: \
                     crawl(callees, entryPoints(lang)).",
                ))
            }
        }
        if matches!(lookup.builtin, Some((Builtin::TypeOf, _))) {
            return Err(Exception::throw_type(
                ctx,
                "crawl(lookups): typeOf maps a node to its type, not to nodes; it can't be crawled.",
            ));
        }
        walk.push(lookup);
    }

    let starts = if Class::<SgNodeRjs>::from_value(&start).is_ok() || start.is_string() {
        vec![start]
    } else if let Ok(lookup) = Class::<Lookup>::from_value(&start) {
        // A lookup as the start: its nodes (`crawl(callees, entryPoints(lang))`).
        let lookup = lookup.borrow();
        lookup
            .list(ctx, "as crawl()'s start")?
            .iter()
            .map(|e| e.node.clone())
            .collect()
    } else {
        iterate_values(ctx, &start, "crawl(lookups, start)")?
    };

    let mut stop: HashSet<String> = HashSet::new();
    let options = match options {
        Some(value) if !value.is_undefined() && !value.is_null() => {
            let Some(object) = value.into_object() else {
                return Err(Exception::throw_type(
                    ctx,
                    "crawl(..., options): options must be an object",
                ));
            };
            // A typo would silently crawl everything: say so instead.
            for key in object.keys::<String>() {
                let key = key?;
                if key != "stopAt" {
                    return Err(Exception::throw_type(
                        ctx,
                        &format!("crawl(..., options): unknown option `{key}`; expected `stopAt`"),
                    ));
                }
            }
            Some(object)
        }
        _ => None,
    };
    if let Some(options) = options.as_ref() {
        let stop_at: Value = options.get("stopAt")?;
        if !stop_at.is_undefined() && !stop_at.is_null() {
            for node in iterate_values(ctx, &stop_at, "crawl(..., { stopAt })")? {
                stop.insert(key_of(ctx, &node, "crawl(..., { stopAt })")?);
            }
        }
    }

    let mut entries: Vec<Stored<'js>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    // A node is walked on from at most twice: before and after the crawl
    // stepped into a callee. Once inside a callee, the crawl does not step
    // out to the callee's other callers: the value comes back to the call
    // it went in by, through that call's own `argument` edge.
    let mut walked: HashSet<(String, bool)> = HashSet::new();
    let mut queue: VecDeque<(String, Value<'js>, bool)> = VecDeque::new();
    let mut nodes = Nodes::default();
    for node in starts {
        let key = key_of(ctx, &node, "crawl(lookups, start)")?;
        if index.contains_key(&key) {
            continue;
        }
        let node = nodes
            .node(ctx, node, "crawl(lookups, start)")?
            .expect("a node or a key");
        index.insert(key.clone(), entries.len());
        entries.push(Stored {
            node: node.clone(),
            value: Value::new_null(ctx.clone()),
        });
        walked.insert((key.clone(), false));
        queue.push_back((key, node, false));
    }
    while let Some((key, node, inside)) = queue.pop_front() {
        if stop.contains(&key) {
            continue;
        }
        for lookup in &walk {
            let (edges, crossings) = lookup.edges_crossing(ctx, &node)?;
            for (at, edge) in array_items(&edges)?.into_iter().enumerate() {
                let crossing = crossings.get(at).copied().flatten();
                if inside && crossing == Some(Crossing::Out) {
                    continue;
                }
                let edge: Object = edge.get()?;
                let target: Value = edge.get("node")?;
                let target_key = key_of(ctx, &target, "crawl()")?;
                if !index.contains_key(&target_key) {
                    let reached = Object::new(ctx.clone())?;
                    for prop in edge.props::<String, Value>() {
                        let (name, value) = prop?;
                        reached.set(name, value)?;
                    }
                    reached.set("from", node.clone())?;
                    index.insert(target_key.clone(), entries.len());
                    entries.push(Stored {
                        node: target.clone(),
                        value: reached.into_value(),
                    });
                }
                // `stop: true`: the value ends here (stored, returned, or
                // passed to a call whose result doesn't carry it).
                if edge.get::<_, Option<bool>>("stop")? == Some(true) {
                    continue;
                }
                let inside = inside || crossing == Some(Crossing::Into);
                if walked.insert((target_key.clone(), inside)) {
                    queue.push_back((target_key, target, inside));
                }
            }
        }
    }
    Ok(Lookup::stored(entries, index, Shape::Reached))
}

/// One side of a `join`: a pattern, the metavariable whose text joins, and
/// which node an edge holds.
struct Side<'js> {
    /// A pattern string, or a rule object as `findAll` takes.
    find: Value<'js>,
    on: String,
    anchor_function: bool,
}

fn side<'js>(ctx: &Ctx<'js>, spec: &Object<'js>, name: &str) -> Result<Side<'js>> {
    let bad = |what: &str| Exception::throw_type(ctx, &format!("join({{ {name} }}): {what}"));
    let object: Option<Object> = spec.get(name)?;
    let object = object.ok_or_else(|| bad("must be { find, on }"))?;
    let find: Value = object.get("find")?;
    if !find.is_string() && !find.is_object() {
        return Err(bad(
            "`find` must be a pattern, or a rule as findAll() takes",
        ));
    }
    let on: Option<String> = object.get("on")?;
    let on = on.ok_or_else(|| bad("`on` must name a metavariable of `find`, like \"$NAME\""))?;
    let Some(var) = on.strip_prefix('$').filter(|v| {
        !v.is_empty()
            && v.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    }) else {
        return Err(bad(&format!(
            "`on` must be one metavariable, like \"$NAME\", and is \"{on}\""
        )));
    };
    if let Some(pattern) = find.as_string() {
        if !pattern.to_string()?.contains(&on) {
            return Err(bad(&format!(
                "`on` is {on}, and `find` does not capture it"
            )));
        }
    }
    let anchor: Option<String> = object.get("anchor")?;
    let anchor_function = match anchor.as_deref() {
        None | Some("match") => false,
        Some("function") => true,
        Some(other) => {
            return Err(bad(&format!(
                "`anchor` is \"match\" (the default) or \"function\", and is \"{other}\""
            )))
        }
    };
    Ok(Side {
        find,
        on: var.to_string(),
        anchor_function,
    })
}

fn source_extensions(family: Family) -> &'static [&'static str] {
    match family {
        Family::JavaScript => &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"],
        Family::Python => &["py"],
    }
}

/// The workspace's source files for `family`, in path order, skipping what
/// `.gitignore` and `node_modules` hold.
fn workspace_sources(root: &std::path::Path, family: Family) -> Vec<std::path::PathBuf> {
    let extensions = source_extensions(family);
    let mut files: Vec<_> = ignore::WalkBuilder::new(root)
        .filter_entry(|entry| entry.file_name() != "node_modules")
        .build()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.contains(&e))
        })
        .collect();
    files.sort();
    files
}

/// How `join` decides two captures name the same thing.
enum By<'js> {
    /// Both resolve: the same declaration. Either does not: the same text.
    Resolve(Lookup<'js>),
    /// The same text.
    Text,
    /// Both resolve, through this lookup, to the same node.
    Only(Lookup<'js>),
}

impl<'js> By<'js> {
    /// The key of the node `capture` resolves to, remembered by capture.
    /// With the default, a capture the analysis can't resolve (or no
    /// analysis) is unresolved, not an error.
    fn resolve(
        &self,
        ctx: &Ctx<'js>,
        capture: &SgNodeRjs<'js>,
        resolved: &mut HashMap<String, Option<String>>,
    ) -> Result<Option<String>> {
        let lookup = match self {
            By::Text => return Ok(None),
            By::Resolve(lookup) | By::Only(lookup) => lookup,
        };
        let key = capture.key();
        if let Some(known) = resolved.get(&key) {
            return Ok(known.clone());
        }
        let node = Class::instance(ctx.clone(), capture.clone())?.into_value();
        let found = match lookup.edges(ctx, &node) {
            Ok(edges) => array_items(&edges)?
                .first()
                .and_then(|edge| edge.as_object().cloned())
                .map(|edge| edge.get::<_, Value>("node"))
                .transpose()?
                .map(|target| key_of(ctx, &target, "join({ by })"))
                .transpose()?,
            Err(rquickjs::Error::Exception) if matches!(self, By::Resolve(_)) => {
                let _ = ctx.catch();
                None
            }
            Err(e) => return Err(e),
        };
        resolved.insert(key, found.clone());
        Ok(found)
    }
}

/// `join({ from, to, meta, language })`: a lookup with an edge from each
/// `from` match to each `to` match whose `on` metavariables captured the same
/// text, built in one pass over the workspace.
pub(crate) fn join_rjs<'js>(ctx: Ctx<'js>, spec: Object<'js>) -> Result<Lookup<'js>> {
    let from = side(&ctx, &spec, "from")?;
    let to = side(&ctx, &spec, "to")?;
    let meta: Option<Object> = spec.get("meta")?;
    if let Some(meta) = &meta {
        for prop in meta.keys::<String>() {
            let name = prop?;
            if matches!(name.as_str(), "node" | "from" | "site" | "name") {
                return Err(Exception::throw_type(
                    &ctx,
                    &format!("join({{ meta }}): `{name}` is set on every edge; pick another name"),
                ));
            }
        }
    }
    let by: Value = spec.get("by")?;
    let language: Option<String> = spec.get("language")?;
    let language = language
        .or_else(|| ctx.userdata::<DefaultParseLanguage>().map(|l| l.0.clone()))
        .ok_or_else(|| {
            Exception::throw_type(
                &ctx,
                "join(): name the language, join({ language: \"tsx\", from, to })",
            )
        })?;
    let family = family_of(&ctx, "join", &language)?;
    let target = target_directory(&ctx).ok_or_else(|| {
        Exception::throw_message(
            &ctx,
            "join(): there is no workspace here. It works inside workflow steps.",
        )
    })?;
    let by = match by.as_string() {
        _ if by.is_undefined() || by.is_null() => By::Resolve(Lookup::builtin(Builtin::Definition, family)),
        Some(text) if text.to_string()? == "text" => By::Text,
        _ => match Class::<Lookup>::from_value(&by) {
            Ok(lookup) if lookup.borrow().shape == Shape::Edges => {
                By::Only(lookup.borrow().clone())
            }
            _ => {
                return Err(Exception::throw_type(
                    &ctx,
                    "join({ by }): \"text\", or a lookup to resolve captures with, like symbols(lang).definition. \
                     Leave it out to resolve where the analysis can and compare text elsewhere.",
                ))
            }
        },
    };
    let provider = step_provider(&ctx);

    // The `from` matches and the `to` matches, each with its joined text.
    // Each with the node its `on` metavariable captured.
    let mut starts: Vec<(String, SgNodeRjs<'js>, SgNodeRjs<'js>, SgNodeRjs<'js>)> = Vec::new();
    let mut targets: HashMap<String, Vec<(SgNodeRjs<'js>, SgNodeRjs<'js>)>> = HashMap::new();
    let mut matchers: HashMap<String, (JsMatcherRjs, JsMatcherRjs)> = HashMap::new();
    for path in workspace_sources(&target, family) {
        let Some(root) = root_for_path(provider.clone(), &path, Some(&target)) else {
            continue;
        };
        let lang = *root.grep.lang();
        let key = lang.to_string();
        if !matchers.contains_key(&key) {
            let compile = |find: &Value<'js>, name: &str| {
                convert_matcher(find.clone(), lang, &ctx).map_err(|_| {
                    let message = ctx
                        .catch()
                        .as_exception()
                        .and_then(|e| e.message())
                        .unwrap_or_default();
                    Exception::throw_type(
                        &ctx,
                        &format!("join({{ {name} }}): `find` is not a pattern or rule: {message}"),
                    )
                })
            };
            matchers.insert(
                key.clone(),
                (compile(&from.find, "from")?, compile(&to.find, "to")?),
            );
        }
        let (from_pattern, to_pattern) = &matchers[&key];
        for found in SgNodeRjs::matches_in(&root, from_pattern) {
            let (Some(text), Some(capture)) =
                (found.captured_text(&from.on), found.captured_node(&from.on))
            else {
                continue;
            };
            let start = if from.anchor_function {
                match found.enclosing_function() {
                    Some(function) => function,
                    None => continue,
                }
            } else {
                found.clone()
            };
            starts.push((text, start, found, capture));
        }
        for found in SgNodeRjs::matches_in(&root, to_pattern) {
            let (Some(text), Some(capture)) =
                (found.captured_text(&to.on), found.captured_node(&to.on))
            else {
                continue;
            };
            let node = if to.anchor_function {
                match found.enclosing_function() {
                    Some(function) => function,
                    None => continue,
                }
            } else {
                found
            };
            targets.entry(text).or_default().push((node, capture));
        }
    }

    let mut entries: Vec<Stored<'js>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut resolved: HashMap<String, Option<String>> = HashMap::new();
    for (text, start, site, capture) in starts {
        let Some(ends) = targets.get(&text) else {
            continue;
        };
        let start_key = start.key();
        let from_declaration = by.resolve(&ctx, &capture, &mut resolved)?;
        for (end, end_capture) in ends {
            let joined = match &by {
                By::Text => true,
                By::Resolve(_) => match (
                    &from_declaration,
                    by.resolve(&ctx, end_capture, &mut resolved)?,
                ) {
                    (Some(a), Some(b)) => *a == b,
                    _ => true,
                },
                By::Only(_) => match (
                    &from_declaration,
                    by.resolve(&ctx, end_capture, &mut resolved)?,
                ) {
                    (Some(a), Some(b)) => *a == b,
                    _ => false,
                },
            };
            if !joined {
                continue;
            }
            if !seen.insert((start_key.clone(), end.key())) {
                continue;
            }
            let edge = Object::new(ctx.clone())?;
            edge.set("node", end.clone())?;
            edge.set("site", site.clone())?;
            edge.set("name", text.clone())?;
            if let Some(meta) = &meta {
                for prop in meta.props::<String, Value>() {
                    let (name, value) = prop?;
                    edge.set(name, value)?;
                }
            }
            let slot = match index.get(&start_key) {
                Some(&i) => i,
                None => {
                    index.insert(start_key.clone(), entries.len());
                    entries.push(Stored {
                        node: Class::instance(ctx.clone(), start.clone())?.into_value(),
                        value: Array::new(ctx.clone())?.into_value(),
                    });
                    entries.len() - 1
                }
            };
            let list = entries[slot].value.as_array().cloned().expect("an array");
            list.set(list.len(), edge)?;
        }
    }
    freeze_all(&ctx, &entries)?;
    Ok(Lookup::stored(entries, index, Shape::Edges))
}

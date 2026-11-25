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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use rquickjs::{
    atom::PredefinedAtom, class::Trace, prelude::Opt, Array, Class, Ctx, Exception, Function,
    IntoJs, JsLifetime, Object, Promise, Result, Value,
};

use super::sg_node::{root_for_path, SgNodeRjs, SgRootInner};
use super::utils::{convert_matcher, JsMatcherRjs};
use super::{DefaultParseLanguage, ParseSemanticProvider};
use crate::sandbox::engine::execution_engine::TargetDirectory;

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
            Self::TypeOf => "typeOf",
        }
    }
}

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
    /// A crawl's result, not a lookup: crawl does not take it.
    #[qjs(skip_trace)]
    reached: bool,
    entries: Vec<Stored<'js>>,
    #[qjs(skip_trace)]
    index: HashMap<String, usize>,
}

unsafe impl<'js> JsLifetime<'js> for Lookup<'js> {
    type Changed<'to> = Lookup<'to>;
}

impl<'js> Lookup<'js> {
    fn builtin(kind: Builtin, family: Family) -> Self {
        Self {
            builtin: Some((kind, family)),
            reached: false,
            entries: Vec::new(),
            index: HashMap::new(),
        }
    }

    fn stored(entries: Vec<Stored<'js>>, index: HashMap<String, usize>, reached: bool) -> Self {
        Self {
            builtin: None,
            reached,
            entries,
            index,
        }
    }

    fn what(&self) -> &'static str {
        match self.builtin {
            Some((kind, _)) => kind.name(),
            None if self.reached => "crawl()'s result",
            None => "lookup()",
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
        let Some((kind, family)) = self.builtin else {
            let key = key_of(ctx, node, self.what())?;
            return Ok(match self.index.get(&key) {
                Some(&i) => self.entries[i].value.clone(),
                None => Value::new_undefined(ctx.clone()),
            });
        };
        let class = node_from_value(ctx, node, kind.name())?;
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
                    sg.callers(ctx.clone(), Opt(None))?
                } else {
                    sg.callees(ctx.clone(), Opt(None))?
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
                let found = if kind == Builtin::Sources {
                    sg.flows_from(ctx.clone(), Opt(None))?
                } else {
                    sg.flows_to(ctx.clone(), Opt(None))?
                };
                for step in array_items(&found)? {
                    let step: Object = step.get()?;
                    if step.get::<_, Option<u32>>("depth")? != Some(1) {
                        continue;
                    }
                    let edge = Object::new(ctx.clone())?;
                    for field in ["node", "kind", "via", "name", "index", "boundary"] {
                        edge.set(field, step.get::<_, Value>(field)?)?;
                    }
                    push(edge)?;
                }
            }
            Builtin::Definition => {
                // Through imports, to the declaration: a lookup's edges land
                // on what the name means, so a crawl can go on from there.
                let options = Object::new(ctx.clone())?;
                options.set("follow", true)?;
                let found = sg.definition(ctx.clone(), Opt(Some(options)))?;
                if let Some(definition) = found.as_object() {
                    let edge = Object::new(ctx.clone())?;
                    for field in ["node", "kind", "exportName"] {
                        edge.set(field, definition.get::<_, Value>(field)?)?;
                    }
                    push(edge)?;
                }
            }
            Builtin::References => {
                let found = sg.references(ctx.clone())?;
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
            Builtin::TypeOf => {
                let found = sg.type_info(ctx.clone())?;
                return Ok(if found.is_null() {
                    Value::new_undefined(ctx.clone())
                } else {
                    found
                });
            }
        }
        if edges.is_empty() {
            return Ok(Value::new_undefined(ctx.clone()));
        }
        edges.into_js(ctx)
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

fn key_of<'js>(ctx: &Ctx<'js>, node: &Value<'js>, what: &str) -> Result<String> {
    let class = node_from_value(ctx, node, what)?;
    let key = class.borrow().key();
    Ok(key)
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

/// `dataflow(lang)`: `{ sources, uses }`.
pub(crate) fn dataflow_rjs<'js>(ctx: Ctx<'js>, lang: String) -> Result<Object<'js>> {
    let family = family_of(&ctx, "dataflow", &lang)?;
    pair(
        &ctx,
        family,
        &[("sources", Builtin::Sources), ("uses", Builtin::Uses)],
    )
}

/// `symbols(lang)`: `{ definition, references }`.
pub(crate) fn symbols_rjs<'js>(ctx: Ctx<'js>, lang: String) -> Result<Object<'js>> {
    let family = family_of(&ctx, "symbols", &lang)?;
    pair(
        &ctx,
        family,
        &[
            ("definition", Builtin::Definition),
            ("references", Builtin::References),
        ],
    )
}

/// `types(lang)`: `{ typeOf }`.
pub(crate) fn types_rjs<'js>(ctx: Ctx<'js>, lang: String) -> Result<Object<'js>> {
    let family = family_of(&ctx, "types", &lang)?;
    if family != Family::JavaScript {
        return Err(Exception::throw_type(
            &ctx,
            &format!("types(\"{lang}\"): types come from the TypeScript 7 checker, for TypeScript and JavaScript."),
        ));
    }
    pair(&ctx, family, &[("typeOf", Builtin::TypeOf)])
}

/// `entryPoints(lang)`: every function in the workspace something outside
/// its code calls, each with `entryPoints()` saying why.
pub(crate) fn entry_points_rjs<'js>(ctx: Ctx<'js>, lang: String) -> Result<Array<'js>> {
    let family = family_of(&ctx, "entryPoints", &lang)?;
    let provider = step_provider(&ctx).ok_or_else(|| {
            Exception::throw_message(
                &ctx,
                "entryPoints(): there is no semantic analysis here. It works inside forEachFile callbacks.",
            )
        })?;
    let found = provider
        .list_entry_points(family.provider_language())
        .map_err(|e| Exception::throw_message(&ctx, &format!("entryPoints(): {e}")))?;
    let target = target_directory(&ctx);
    let result = Array::new(ctx.clone())?;
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
        if let Some(node) = SgNodeRjs::node_at(&root, function.range) {
            result.set(result.len(), node)?;
        }
    }
    Ok(result)
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

/// `lookup(edges)`: a lookup from `[from, to, meta?]` edges.
pub(crate) fn lookup_rjs<'js>(ctx: Ctx<'js>, edges: Value<'js>) -> Result<Lookup<'js>> {
    let mut entries: Vec<Stored<'js>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (n, item) in iterate_values(&ctx, &edges, "lookup(edges)")?
        .into_iter()
        .enumerate()
    {
        let bad =
            |what: &str| Exception::throw_type(&ctx, &format!("lookup(edges): edge {n} {what}"));
        let parts = item
            .as_array()
            .ok_or_else(|| bad("must be [from, to] or [from, to, meta]"))?;
        let from: Value = parts.get(0)?;
        let to: Value = parts.get(1)?;
        let meta: Value = parts.get(2)?;
        let from_key = node_key_or(&ctx, &from, || bad("has a `from` that is not a node"))?;
        node_key_or(&ctx, &to, || bad("has a `to` that is not a node"))?;
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
    freeze_all(&ctx, &entries)?;
    Ok(Lookup::stored(entries, index, false))
}

fn node_key_or<'js>(
    ctx: &Ctx<'js>,
    value: &Value<'js>,
    bad: impl FnOnce() -> rquickjs::Error,
) -> Result<String> {
    match Class::<SgNodeRjs>::from_value(value) {
        Ok(class) => Ok(class.borrow().key()),
        Err(_) => {
            let _ = ctx;
            Err(bad())
        }
    }
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
            Exception::throw_type(
                ctx,
                "crawl(lookups): each lookup must be a built-in lookup or one lookup(edges) made. \
                 Make a Map into one with lookup([...map].flatMap(([from, tos]) => tos.map((to) => [from, to]))).",
            )
        })?;
        let lookup = lookup.borrow().clone();
        if lookup.reached {
            return Err(Exception::throw_type(
                ctx,
                "crawl(lookups): a crawl's result is not a lookup.",
            ));
        }
        if matches!(lookup.builtin, Some((Builtin::TypeOf, _))) {
            return Err(Exception::throw_type(
                ctx,
                "crawl(lookups): typeOf maps a node to its type, not to nodes; it can't be crawled.",
            ));
        }
        walk.push(lookup);
    }

    let starts = if Class::<SgNodeRjs>::from_value(&start).is_ok() {
        vec![start]
    } else {
        iterate_values(ctx, &start, "crawl(lookups, start)")?
    };

    let mut stop: HashSet<String> = HashSet::new();
    if let Some(options) = options.as_ref().and_then(Value::as_object) {
        let stop_at: Value = options.get("stopAt")?;
        if !stop_at.is_undefined() && !stop_at.is_null() {
            for node in iterate_values(ctx, &stop_at, "crawl(..., { stopAt })")? {
                stop.insert(key_of(ctx, &node, "crawl(..., { stopAt })")?);
            }
        }
    }

    let mut entries: Vec<Stored<'js>> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut queue: VecDeque<(String, Value<'js>)> = VecDeque::new();
    for node in starts {
        let key = key_of(ctx, &node, "crawl(lookups, start)")?;
        if index.contains_key(&key) {
            continue;
        }
        index.insert(key.clone(), entries.len());
        entries.push(Stored {
            node: node.clone(),
            value: Value::new_null(ctx.clone()),
        });
        queue.push_back((key, node));
    }
    while let Some((key, node)) = queue.pop_front() {
        if stop.contains(&key) {
            continue;
        }
        for lookup in &walk {
            let edges = lookup.edges(ctx, &node)?;
            for edge in array_items(&edges)? {
                let edge: Object = edge.get()?;
                let target: Value = edge.get("node")?;
                let target_key = key_of(ctx, &target, "crawl()")?;
                if index.contains_key(&target_key) {
                    continue;
                }
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
                queue.push_back((target_key, target));
            }
        }
    }
    Ok(Lookup::stored(entries, index, true))
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
            Ok(lookup) if !lookup.borrow().reached => By::Only(lookup.borrow().clone()),
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
    Ok(Lookup::stored(entries, index, false))
}

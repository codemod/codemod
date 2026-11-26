//! workflow.yaml compatibility: translates a YAML workflow into the
//! equivalent TypeScript workflow. `workflow run` uses this in memory;
//! `codemod workflow upgrade` writes the result next to the YAML, with each
//! jssg transform called from a plain `forEachFile` callback where it can be
//! (see [`transform_module`]), where `workflow run` uses the compatibility
//! adapter for all of them. The output is printed in Prettier's style
//! ([`print`]), with selectors several steps share as one const.
//!
//! - a workflow with one plain node becomes a `main` that runs its steps;
//!   otherwise each node becomes an async function, run in order when the
//!   nodes form a chain, together when they are independent, and through a
//!   promise graph for anything else, so independent nodes still run at the
//!   same time;
//! - each step becomes `await step("<name>", <task>)`;
//! - matrix nodes run once per value (`values`, or the array in state for
//!   `from_state`), concurrently, or one value at a time when the node uses
//!   git steps: those share one checkout and refuse to run alongside others;
//! - workflow state is a plain object local to the run, `state`. A shell
//!   step gets it as `CODEMOD_STATE_<KEY>` variables, and the lines it writes
//!   to `$STATE_OUTPUTS` are printed at the end of its stdout, from where
//!   `applyStateOutputs` copies them into `state`. This shell transport
//!   requires POSIX; workflows that use it fail before starting on Windows.
//!   The engine has no workflow state; a jssg transform
//!   can neither read nor set it, so the YAML engine's `getState`/`setState`
//!   in a transform cannot be translated;
//! - `${{ }}` expressions become TypeScript where that is exact, and
//!   otherwise call `evaluate(...)`, which uses the YAML engine's evaluator;
//!   `if:` conditions always go through `evaluate(...)`, and one that cannot
//!   be evaluated skips its step, as in the YAML engine;
//! - `trigger: manual` becomes an `approval` step; `branch_name`,
//!   `pull_request` and step `commit` become git steps.

pub mod expr;
mod print;
mod transform_module;
mod v1;

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use butterflow_models::node::NodeType;
use butterflow_models::step::{
    SemanticAnalysisConfig, SemanticAnalysisMode, ShardMethod, StepAction, UseJSAstGrep,
};
use butterflow_models::trigger::TriggerType;
use butterflow_models::{Node, Step, Workflow};
use serde_json::Value;

use expr::{condition, js_string, string_list, template};
use transform_module::{Analysis, TransformModule};

/// A translated workflow.
#[derive(Debug, Clone)]
pub struct Translated {
    pub source: String,
    /// Features that have no effect after translation.
    pub warnings: Vec<String>,
}

/// Parse, validate and translate a workflow.yaml (or JSON) file, as
/// `workflow run` does: jssg transforms run through the compatibility
/// adapter.
pub fn translate_file(path: &Path) -> Result<Translated, String> {
    let workflow = parse_and_validate(path)?;
    refuse_containers(&workflow).map_err(|e| format!("{}: {e}", path.display()))?;
    let usage = v1::usage(path.parent().unwrap_or(Path::new(".")));
    let mut generator = Generator {
        v1_state: usage.state,
        v1_metrics: usage.metrics,
        ..Generator::default()
    };
    generator.workflow(&workflow);
    Ok(generator.finish())
}

/// `runtime: docker` and `runtime: podman` ran a node's steps in a
/// container in codemod 1.x. 2.0 runs no containers, and running those steps
/// on this machine instead would drop the isolation the package asked for,
/// so such a workflow does not run.
fn refuse_containers(workflow: &Workflow) -> Result<(), String> {
    let contained = |runtime: Option<&butterflow_models::Runtime>| {
        runtime
            .filter(|r| r.r#type != butterflow_models::RuntimeType::Direct)
            .map(|r| r.r#type.to_string())
    };
    let found = workflow
        .nodes
        .iter()
        .find_map(|node| {
            contained(node.runtime.as_ref()).map(|kind| (format!("node '{}'", node.id), kind))
        })
        .or_else(|| {
            workflow.templates.iter().find_map(|template| {
                contained(template.runtime.as_ref())
                    .map(|kind| (format!("template '{}'", template.id), kind))
            })
        });
    match found {
        Some((what, kind)) => Err(format!(
            "{what} runs its steps in a {kind} container (`runtime: {kind}`), which codemod 2.0 does not do. Run this package with codemod 1.x (npx codemod@1 ...), or remove `runtime` from the workflow to run its steps directly on this machine"
        )),
        None => Ok(()),
    }
}

/// Translate a workflow.yaml (or JSON) file into the workflow.ts
/// `codemod workflow upgrade` writes: a jssg transform is imported and
/// called from a plain `forEachFile` callback, unless it needs what only
/// the compatibility adapter does (see [`transform_module`]).
pub fn upgrade_file(path: &Path) -> Result<Translated, String> {
    let workflow = parse_and_validate(path)?;
    let bundle = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut generator = Generator {
        upgrade: Some(bundle),
        ..Generator::default()
    };
    generator.workflow(&workflow);
    Ok(generator.finish())
}

fn parse_and_validate(path: &Path) -> Result<Workflow, String> {
    let workflow = butterflow_core::utils::parse_workflow_file(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let bundle = path.parent().unwrap_or(Path::new("."));
    butterflow_core::utils::validate_workflow(&workflow, bundle)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(workflow)
}

/// Translate an already-parsed workflow.
pub fn translate(workflow: &Workflow) -> Translated {
    let mut generator = Generator::default();
    generator.workflow(workflow);
    generator.finish()
}

#[derive(Default)]
struct Generator {
    body: String,
    warnings: Vec<String>,
    imports: BTreeSet<&'static str>,
    uses_after: bool,
    uses_matrix_env: bool,
    uses_context: bool,
    /// Shell steps get state as variables and copy what they set into it:
    /// the workflow reads state, or has more than one step that could set it.
    plumb_state: bool,
    uses_state_env: bool,
    /// Whether a generated command needs the POSIX `$STATE_OUTPUTS` wrapper.
    uses_posix_state_outputs: bool,
    /// Whether the source calls `applyStateOutputs`.
    uses_state_outputs: bool,
    /// Tasks get the run's params anyway; pass `params` explicitly only when
    /// it differs from them (defaults, template inputs).
    pass_params: bool,
    names: HashMap<String, String>,
    taken: BTreeSet<String>,
    /// Shard functions and ast-grep rule files the workflow imports, in
    /// the order first used: (local name, import specifier).
    transforms: Vec<(String, String)>,
    /// jssg transform modules, imported as namespaces for the compatibility
    /// adapter: (local name, import specifier).
    jssg_modules: Vec<(String, String)>,
    /// What the callbacks import from the compatibility adapter.
    adapter: BTreeSet<&'static str>,
    /// For `codemod workflow upgrade`: the workflow's directory, where the
    /// jssg transforms are read to call them from plain callbacks.
    upgrade: Option<PathBuf>,
    /// What each jssg transform needs, by `js_file`.
    analyses: HashMap<String, Analysis>,
    /// Transforms called from plain callbacks: (local name, specifier, and
    /// the local name of its `getSelector` when that is imported too).
    plain_transforms: Vec<(String, String, Option<String>)>,
    /// The workflow has one jssg transform, imported as `transform`.
    single_transform: bool,
    /// Templates take the run's target (a transform there reads `targetDir`).
    templates_take_target: bool,
    /// Transforms whose fallback to the adapter has been explained.
    adapter_warned: BTreeSet<String>,
    /// A plain callback parses with `codemod/ast-grep`.
    uses_parse: bool,
    /// The workflow's param defaults are all strings (or there are none).
    string_defaults: bool,
    /// A transform or `getSelector` gets `params`, typed as jssg types it.
    params_to_transforms: bool,
    /// `forEachFile` selectors, in the order used, with how often: each is
    /// written at its place or, when shared, as a const.
    selectors: Vec<(String, usize)>,
    /// Codemod 1.x compatibility (`workflow run` only): the package's
    /// transforms use `codemod:workflow`'s state functions, so jssg steps get
    /// the workflow's state and their result's state goes back into it.
    v1_state: bool,
    /// The package's transforms count with `codemod:metrics`: the counts
    /// are added up and returned from the run.
    v1_metrics: bool,
}

/// Object fields: (key, TypeScript value).
type Fields = Vec<(&'static str, String)>;

/// One node, generated before deciding how `main` runs it.
struct NodeCode {
    function: String,
    /// Doc comment and warnings, without indentation.
    preamble: String,
    /// The node's statements, indented for a function body.
    inner: String,
    /// Matrix values (`values` or state) when the node runs once per value.
    matrix: Option<String>,
    /// The node has git steps, so its matrix values run one at a time.
    uses_git: bool,
    uses_params: bool,
    uses_target: bool,
    uses_state: bool,
}

/// Parameters of a node function: `params`, `target` and `state` when it
/// uses them.
fn parameters(code: &NodeCode) -> Vec<&'static str> {
    let mut names = Vec::new();
    if code.uses_params {
        names.push("params");
    }
    if code.uses_target {
        names.push("target");
    }
    if code.uses_state {
        names.push("state");
    }
    names
}

fn declared(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|name| match *name {
            "params" => "params: Params".to_string(),
            "target" => "target: string".to_string(),
            _ => "state: State".to_string(),
        })
        .collect()
}

/// Whether generated code passes the run's target on: to a transform's
/// options, or to a template.
fn uses_target(text: &str) -> bool {
    text.contains("targetDir: target") || text.contains(", target)")
}

/// Whether translated steps must carry state from one to the next: the
/// workflow reads it (`state.` in an expression, `from_state`), or more than
/// one step could set it (shell, shard), so a later one may read it.
/// A template's steps count once for each time it is used.
fn plumbs_state(workflow: &Workflow) -> bool {
    let text = serde_json::to_string(workflow).unwrap_or_default();
    let reads = text.contains("state.")
        || workflow.nodes.iter().any(|node| {
            node.strategy
                .as_ref()
                .is_some_and(|s| s.from_state.is_some())
        });
    let mut counted = HashMap::new();
    let writers: usize = workflow
        .nodes
        .iter()
        .map(|node| writers(&node.steps, workflow, &mut Vec::new(), &mut counted))
        .fold(0, usize::saturating_add);
    reads || writers > 1
}

/// How many steps among these could set state, following template uses.
/// `using` holds the templates being followed, so a template that uses
/// itself (an error elsewhere) ends the walk instead of looping; `counted`
/// keeps each template's count, so a template is walked once however often
/// it is used.
fn writers<'a>(
    steps: &[Step],
    workflow: &'a Workflow,
    using: &mut Vec<&'a str>,
    counted: &mut HashMap<&'a str, usize>,
) -> usize {
    steps
        .iter()
        .map(|step| match &step.action {
            StepAction::RunScript(_) | StepAction::Shard(_) => 1,
            StepAction::UseTemplate(used) => {
                let Some(template) = workflow
                    .templates
                    .iter()
                    .find(|template| template.id == used.template)
                    .filter(|template| !using.contains(&template.id.as_str()))
                else {
                    return 0;
                };
                if let Some(&count) = counted.get(template.id.as_str()) {
                    return count;
                }
                using.push(&template.id);
                let count = writers(&template.steps, workflow, using, counted);
                using.pop();
                counted.insert(&template.id, count);
                count
            }
            _ => 0,
        })
        .fold(0, usize::saturating_add)
}

/// Whether a node has git steps: a branch, a pull request, or a step commit,
/// its own or in a template it uses.
fn node_uses_git(node: &Node, workflow: &Workflow) -> bool {
    node.branch_name.is_some()
        || node.pull_request.is_some()
        || steps_commit(&node.steps, workflow, &mut Vec::new(), &mut HashMap::new())
}

/// Whether steps commit, themselves or in the templates they use. Like
/// `writers`, a template is walked once however often it is used.
fn steps_commit<'a>(
    steps: &[Step],
    workflow: &'a Workflow,
    using: &mut Vec<&'a str>,
    checked: &mut HashMap<&'a str, bool>,
) -> bool {
    steps.iter().any(|step| {
        if step.commit.is_some() {
            return true;
        }
        let StepAction::UseTemplate(used) = &step.action else {
            return false;
        };
        let Some(template) = workflow
            .templates
            .iter()
            .find(|template| template.id == used.template)
            .filter(|template| !using.contains(&template.id.as_str()))
        else {
            return false;
        };
        if let Some(&commits) = checked.get(template.id.as_str()) {
            return commits;
        }
        using.push(&template.id);
        let commits = steps_commit(&template.steps, workflow, using, checked);
        using.pop();
        checked.insert(&template.id, commits);
        commits
    })
}

/// Where a list of steps is being generated.
/// What the translation imports from `codemod/git`.
const GIT_STEPS: &[&str] = &["branch", "commit", "pullRequest"];

struct Scope<'a> {
    env: &'a HashMap<String, String>,
    /// A matrix node fed from state: steps default to the shard's files.
    meta_files: bool,
    matrix: bool,
}

impl Generator {
    fn import(&mut self, name: &'static str) -> &'static str {
        self.imports.insert(name);
        name
    }

    /// The local name of the transform (or shard function) at `path`,
    /// relative to the workflow, imported once however often it is used.
    fn transform(&mut self, path: &str, suffix: &str) -> String {
        let specifier = crate::imported::import_specifier(path);
        if let Some((name, _)) = self.transforms.iter().find(|(_, s)| *s == specifier) {
            return name.clone();
        }
        let name = format!(
            "{}{suffix}",
            crate::imported::camel_stem(&specifier).unwrap_or_else(|| "imported".into())
        );
        let mut unique = name.clone();
        let mut n = 2;
        while !self.taken.insert(unique.clone()) {
            unique = format!("{name}{n}");
            n += 1;
        }
        self.transforms.push((unique.clone(), specifier));
        unique
    }

    /// The local name of the jssg transform module at `path`, imported as a
    /// namespace (its `default` and `getSelector`) for the adapter.
    fn jssg_module(&mut self, path: &str) -> String {
        let specifier = crate::imported::import_specifier(path);
        if let Some((name, _)) = self.jssg_modules.iter().find(|(_, s)| *s == specifier) {
            return name.clone();
        }
        let name = format!(
            "{}Transform",
            crate::imported::camel_stem(&specifier).unwrap_or_else(|| "imported".into())
        );
        let mut unique = name.clone();
        let mut n = 2;
        while !self.taken.insert(unique.clone()) {
            unique = format!("{name}{n}");
            n += 1;
        }
        self.jssg_modules.push((unique.clone(), specifier));
        unique
    }

    /// The selector for a jssg step, and the adapter's options.
    fn jssg_selector(&mut self, js: &UseJSAstGrep, scope: &Scope<'_>) -> (Fields, Fields) {
        let mut selector = Vec::new();
        let mut options = Vec::new();
        for (key, value) in self.jssg_fields(js, scope) {
            match key {
                // Exact files, as globs.
                "include" | "files" => selector.push(("include", value)),
                // The adapter parses with the selector's language.
                "exclude" | "semanticAnalysis" | "capabilities" | "language" => {
                    selector.push((key, value))
                }
                "basePath" | "dryRun" | "params" | "matrix" => options.push((key, value)),
                // Workers run files in parallel already.
                _ => {}
            }
        }
        if js.language.is_none() {
            // workflow.yaml's default, where forEachFile's is the package's.
            selector.push(("language", js_string("typescript")));
        }
        (selector, options)
    }

    /// `(file) => jssgFile(module, file, { ... })`: the callback that runs
    /// a jssg transform as the YAML engine did.
    fn jssg_adapter(
        &mut self,
        js: &UseJSAstGrep,
        mut options: Vec<(&'static str, String)>,
        adapter: &'static str,
    ) -> String {
        self.adapter.insert(adapter);
        let module = self.jssg_module(&js.js_file);
        if !options.iter().any(|(key, _)| *key == "params") {
            // The run's params, as a jssg step got them.
            options.push(("params", "params".into()));
        }
        with_options(format!("(file) => {adapter}({module}, file"), &options)
    }

    /// For `codemod workflow upgrade`: the transform at `js_file` as a
    /// plain callback can call it, or why it keeps the adapter (`None`
    /// when not upgrading).
    fn plain_module(&mut self, js: &UseJSAstGrep) -> Result<TransformModule, Option<String>> {
        let Some(bundle) = &self.upgrade else {
            return Err(None);
        };
        let path = bundle.join(&js.js_file);
        let analysis = self
            .analyses
            .entry(js.js_file.clone())
            .or_insert_with(|| transform_module::analyze(&path))
            .clone();
        let module = match analysis {
            Analysis::Plain(module) => module,
            Analysis::Adapter(reason) => return Err(Some(reason)),
        };
        if js.dry_run == Some(true) {
            return Err(Some("the step is a dry run".into()));
        }
        if js
            .base_path
            .as_deref()
            .is_some_and(|base| base.contains("${{"))
        {
            return Err(Some("its base_path is an expression".into()));
        }
        Ok(module)
    }

    /// `(file) => { const out = transform(parse(file)); ... }`: the jssg
    /// transform called as the adapter would, from a plain callback.
    fn jssg_plain(
        &mut self,
        js: &UseJSAstGrep,
        scope: &Scope<'_>,
        module: &TransformModule,
    ) -> String {
        self.uses_parse = true;
        let (name, selector) = self.plain_transform(&js.js_file, module.get_selector);
        let mut statements = Vec::new();
        if let Some(base) = js.base_path.as_deref().filter(|b| !is_root(b)) {
            // Only files under the base, as the YAML engine ran it.
            let base = base.replace('\\', "/");
            let base = base.trim().trim_start_matches("./").trim_end_matches('/');
            statements.push(format!(
                "if (!file.path.startsWith({})) return",
                js_string(&format!("{base}/"))
            ));
        }
        let root = match &selector {
            Some(selector) => {
                self.params_to_transforms = true;
                // Typed as the transform takes it: `parse` alone does not know the language.
                statements.push(format!(
                    "const root: Parameters<typeof {name}>[0] = parse(file)"
                ));
                statements.push(format!(
                    "const matches = root.root().findAll({selector}({{ params }}))"
                ));
                statements.push("if (matches.length === 0) return".into());
                "root"
            }
            None => "parse(file)",
        };
        let mut options: Vec<(&str, String)> = Vec::new();
        if module.takes_options {
            self.params_to_transforms = true;
            options.push(("params", "params".into()));
            options.push((
                "language",
                js_string(js.language.as_deref().unwrap_or("typescript")),
            ));
            options.push(("targetDir", "target".into()));
            if selector.is_some() {
                options.push(("matches", "matches".into()));
            }
            if scope.matrix {
                options.push(("matrixValues", "matrix ?? undefined".into()));
            }
        }
        let arguments = if options.is_empty() {
            root.to_string()
        } else {
            format!("{root}, {}", object(&options))
        };
        let call = format!(
            "{}{name}({arguments})",
            if module.is_async { "await " } else { "" }
        );
        if module.returns_result {
            statements.push(format!("const result = {call}"));
            statements.push("if (typeof result === \"string\") return file.write(result)".into());
            statements
                .push("if (typeof result?.edit === \"string\") file.write(result.edit)".into());
            statements.push("return result?.data".into());
        } else {
            statements.push(format!("const out = {call}"));
            statements.push("if (typeof out === \"string\") file.write(out)".into());
        }
        format!(
            "{}(file) => {{ {}; }}",
            if module.is_async { "async " } else { "" },
            statements.join("; ")
        )
    }

    /// The local name of a transform called from plain callbacks, imported
    /// by default, and of its `getSelector` when it is used.
    fn plain_transform(&mut self, path: &str, with_selector: bool) -> (String, Option<String>) {
        let specifier = crate::imported::import_specifier(path);
        let stem = crate::imported::camel_stem(&specifier).unwrap_or_else(|| "imported".into());
        let index = match self
            .plain_transforms
            .iter()
            .position(|(_, s, _)| *s == specifier)
        {
            Some(index) => index,
            None => {
                let base = if self.single_transform {
                    "transform".to_string()
                } else {
                    format!("{stem}Transform")
                };
                let name = self.unique(&base);
                self.plain_transforms.push((name, specifier, None));
                self.plain_transforms.len() - 1
            }
        };
        if with_selector && self.plain_transforms[index].2.is_none() {
            let selector = if self.single_transform {
                self.unique("getSelector")
            } else {
                self.unique(&format!("{stem}Selector"))
            };
            self.plain_transforms[index].2 = Some(selector);
        }
        let (name, _, selector) = &self.plain_transforms[index];
        (name.clone(), selector.clone().filter(|_| with_selector))
    }

    fn unique(&mut self, name: &str) -> String {
        let mut unique = name.to_string();
        let mut n = 2;
        while !self.taken.insert(unique.clone()) {
            unique = format!("{name}{n}");
            n += 1;
        }
        unique
    }

    /// `body` with its selectors in place: a selector used by more than
    /// one step is a const, written to `source`.
    fn selectors_in(&mut self, mut body: String, source: &mut String) -> String {
        let shared: Vec<usize> = (0..self.selectors.len())
            .filter(|index| {
                let (text, uses) = &self.selectors[*index];
                *uses > 1 && is_literal_object(text)
            })
            .collect();
        let mut consts = String::new();
        for (index, (text, _)) in self.selectors.clone().into_iter().enumerate() {
            let value = if shared.contains(&index) {
                let name = if shared.len() == 1 {
                    self.unique("files")
                } else {
                    match language_of(&text).filter(|language| !language.is_empty()) {
                        Some(language) => self.unique(&format!("{language}Files")),
                        None => self.unique("files"),
                    }
                };
                // `as const` keeps capabilities and languages their literal types.
                let constant = if text.contains("capabilities:")
                    || text.contains("semanticAnalysis:")
                    || !text.contains("include:")
                {
                    " as const"
                } else {
                    ""
                };
                let _ = writeln!(consts, "const {name} = {text}{constant};");
                name
            } else {
                text
            };
            body = body.replace(&format!("{SELECTOR_MARK}{index}\u{1}"), &value);
        }
        if !consts.is_empty() {
            source.push_str(&consts);
            source.push('\n');
        }
        body
    }

    /// `text` with its selectors written out.
    fn with_selectors(&self, text: &str) -> String {
        let mut text = text.to_string();
        for (index, (selector, _)) in self.selectors.iter().enumerate() {
            text = text.replace(&format!("{SELECTOR_MARK}{index}\u{1}"), selector);
        }
        text
    }

    /// `forEachFile(selector, transform)`.
    fn for_each_file(&mut self, selector: &[(&str, String)], transform: String) -> String {
        if selector.is_empty() {
            return format!("forEachFile({transform})");
        }
        let text = object(selector);
        let index = match self.selectors.iter().position(|(known, _)| *known == text) {
            Some(index) => index,
            None => {
                self.selectors.push((text, 0));
                self.selectors.len() - 1
            }
        };
        self.selectors[index].1 += 1;
        format!("forEachFile({SELECTOR_MARK}{index}\u{1}, {transform})")
    }

    fn warn(&mut self, message: String) -> String {
        self.warnings.push(message.clone());
        format!("// codemod upgrade: {}\n", comment_text(&message))
    }

    fn function_name(&mut self, kind: &str, id: &str) -> String {
        let key = format!("{kind}:{id}");
        if let Some(name) = self.names.get(&key) {
            return name.clone();
        }
        let mut name = String::new();
        let mut upper = false;
        for c in id.chars() {
            if c.is_ascii_alphanumeric() {
                if upper && !name.is_empty() {
                    name.push(c.to_ascii_uppercase());
                } else {
                    name.push(c);
                }
                upper = false;
            } else {
                upper = true;
            }
        }
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            name.insert(0, '_');
        }
        name.push_str(kind);
        let mut unique = name.clone();
        let mut n = 2;
        while !self.taken.insert(unique.clone()) {
            unique = format!("{name}{n}");
            n += 1;
        }
        self.names.insert(key, unique.clone());
        unique
    }

    fn workflow(&mut self, workflow: &Workflow) {
        self.import("step");
        if workflow.state.is_some() {
            self.warn("state.schema is not used: workflow state is not validated".into());
        }
        let defaults = workflow
            .params
            .as_ref()
            .map(|params| {
                butterflow_models::schema::resolve_values_with_default(
                    &params.schema,
                    &HashMap::new(),
                )
            })
            .unwrap_or_default();
        let defaults: serde_json::Map<String, Value> = defaults.into_iter().collect();
        let defaults: Value = Value::Object(
            defaults
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        );

        let has_defaults = defaults.as_object().is_some_and(|d| !d.is_empty());
        self.pass_params = has_defaults || !workflow.templates.is_empty();
        // 1.x transforms and shell steps shared one state.
        self.plumb_state = plumbs_state(workflow) || self.v1_state;
        self.string_defaults = defaults
            .as_object()
            .is_none_or(|d| d.values().all(Value::is_string));
        if self.upgrade.is_some() {
            let transforms: Vec<&UseJSAstGrep> = jssg_steps(workflow);
            let files: BTreeSet<&str> = transforms.iter().map(|js| js.js_file.as_str()).collect();
            self.single_transform = files.len() == 1;
            let mut template_transforms = Vec::new();
            for template in &workflow.templates {
                collect_jssg(&template.steps, &mut template_transforms);
            }
            self.templates_take_target = template_transforms.into_iter().any(|js| {
                self.plain_module(js)
                    .is_ok_and(|module| module.takes_options)
            });
        }

        let order = topological(&workflow.nodes);
        let nodes: Vec<NodeCode> = order
            .iter()
            .map(|node| {
                let mut code = self.node_code(node);
                code.uses_git = node_uses_git(node, workflow);
                code
            })
            .collect();

        let mut templates_text = String::new();
        for template_def in &workflow.templates {
            let name = self.function_name("Template", &template_def.id);
            if !template_def.outputs.is_empty()
                || !template_def.env.is_empty()
                || template_def.runtime.is_some()
            {
                let note = self.warn(format!(
                    "template '{}': outputs, env and runtime had no effect and are dropped",
                    template_def.id
                ));
                templates_text.push('\n');
                templates_text.push_str(&note);
            } else {
                templates_text.push('\n');
            }
            let target = if self.templates_take_target {
                ", target: string"
            } else {
                ""
            };
            let _ = writeln!(
                templates_text,
                "/** {} */\nasync function {name}(params: Params, matrix: Matrix, state: State{target}) {{",
                template_def.name
            );
            let empty = HashMap::new();
            let steps = self.steps(
                &template_def.steps,
                &Scope {
                    env: &empty,
                    meta_files: false,
                    matrix: true,
                },
                &workflow_templates(workflow),
            );
            templates_text.push_str(&steps);
            templates_text.push_str("}\n");
        }

        let uses_params = has_defaults
            || !workflow.templates.is_empty()
            || nodes.iter().any(|node| node.uses_params);
        let uses_state = !workflow.templates.is_empty() || nodes.iter().any(|node| node.uses_state);
        let mut main = String::new();
        let platform_field = if self.uses_posix_state_outputs {
            ", platform"
        } else {
            ""
        };
        let target_field = if nodes.iter().any(|node| node.uses_target) {
            ", target"
        } else {
            ""
        };
        if has_defaults {
            self.uses_context = true;
            let _ = writeln!(main, "export default async function main({{ params: input{target_field}{platform_field} }}: WorkflowContext<Params>) {{");
            main.push_str("  const params: Params = { ...DEFAULT_PARAMS, ...input };\n");
        } else if uses_params {
            self.uses_context = true;
            let _ = writeln!(main, "export default async function main({{ params{target_field}{platform_field} }}: WorkflowContext<Params>) {{");
        } else if self.uses_posix_state_outputs || !target_field.is_empty() {
            self.uses_context = true;
            let fields = format!("{target_field}{platform_field}");
            let _ = writeln!(
                main,
                "export default async function main({{ {} }}: WorkflowContext) {{",
                fields.trim_start_matches(", ")
            );
        } else {
            main.push_str("export default async function main() {\n");
        }
        if self.uses_posix_state_outputs {
            // Check the execution host, not the host that ran `workflow upgrade`.
            // An external script may use STATE_OUTPUTS without naming it in YAML.
            main.push_str(concat!(
                "  if (platform === \"win32\") {\n",
                "    throw new Error(\"This translated YAML workflow requires POSIX shell state ",
                "output ($STATE_OUTPUTS), which is not supported on Windows. ",
                "Run it on a POSIX host or migrate its shell steps to workflow.ts.\");\n",
                "  }\n",
            ));
        }
        if uses_state {
            main.push_str("  const state: State = {};\n");
        }

        let call = |node: &NodeCode| format!("{}({})", node.function, parameters(node).join(", "));
        let single = nodes.len() == 1 && nodes[0].matrix.is_none() && workflow.templates.is_empty();
        let chain = order.iter().enumerate().all(|(index, node)| match index {
            0 => node.depends_on.is_empty(),
            _ => node.depends_on.len() == 1 && node.depends_on[0] == order[index - 1].id,
        });
        let independent = order.iter().all(|node| node.depends_on.is_empty());
        if single {
            main.push_str(&indent(
                &nodes[0].preamble.replace("/** ", "// ").replace(" */", ""),
                "  ",
            ));
            main.push_str(&nodes[0].inner);
        } else if chain {
            for node in &nodes {
                let _ = writeln!(main, "  await {};", call(node));
            }
        } else if independent {
            let calls: Vec<String> = nodes.iter().map(&call).collect();
            let _ = writeln!(main, "  await Promise.all([{}]);", calls.join(", "));
        } else {
            let mut started = Vec::new();
            for (node, code) in order.iter().zip(&nodes) {
                let variable = format!("{}Done", code.function);
                if node.depends_on.is_empty() {
                    let _ = writeln!(main, "  const {variable} = {};", call(code));
                } else {
                    self.uses_after = true;
                    let dependencies: Vec<String> = node
                        .depends_on
                        .iter()
                        .map(|id| format!("{}Done", self.function_name("Node", id)))
                        .collect();
                    let _ = writeln!(
                        main,
                        "  const {variable} = after([{}], () => {});",
                        dependencies.join(", "),
                        call(code)
                    );
                }
                started.push(variable);
            }
            let _ = writeln!(main, "  await Promise.all([{}]);", started.join(", "));
        }
        if self.v1_metrics {
            // What 1.x printed at the end of a run.
            main.push_str("  return v1Report();\n");
        }
        main.push_str("}\n");
        self.body.push_str(&main);

        if !single {
            for code in &nodes {
                self.body.push('\n');
                self.body.push_str(&code.preamble);
                let names = parameters(code);
                let params = declared(&names).join(", ");
                match &code.matrix {
                    Some(source) => {
                        let instance = format!("{}For", code.function);
                        let pass: String = names.iter().map(|name| format!("{name}, ")).collect();
                        if code.uses_git {
                            let _ = writeln!(
                                self.body,
                                "async function {}({params}) {{\n  // Git steps share one checkout: one value at a time.\n  const values: Matrix[] = {source};\n  if (!Array.isArray(values)) {{\n    throw new Error(\"matrix values must be a list\");\n  }}\n  for (let index = 0; index < values.length; index++) {{\n    await {instance}({pass}values[index], index);\n  }}\n}}\n",
                                code.function
                            );
                        } else {
                            let _ = writeln!(
                                self.body,
                                "async function {}({params}) {{\n  await Promise.all({source}.map((matrix: Matrix, index: number) => {instance}({pass}matrix, index)));\n}}\n",
                                code.function
                            );
                        }
                        let declare: String = declared(&names)
                            .into_iter()
                            .map(|param| format!("{param}, "))
                            .collect();
                        let _ = writeln!(
                            self.body,
                            "async function {instance}({declare}matrix: Matrix, index: number) {{"
                        );
                    }
                    None => {
                        let _ =
                            writeln!(self.body, "async function {}({params}) {{", code.function);
                    }
                }
                self.body.push_str(&code.inner);
                self.body.push_str("}\n");
            }
        }
        self.body.push_str(&templates_text);

        if has_defaults {
            let header = format!(
                "const DEFAULT_PARAMS: Params = {};\n\n",
                json_literal(&defaults)
            );
            self.body.insert_str(0, &header);
        }
    }

    fn node_code(&mut self, node: &Node) -> NodeCode {
        let function = self.function_name("Node", &node.id);
        let templates = HashMap::new();
        let mut preamble = String::new();
        if let Some(runtime) = node
            .runtime
            .as_ref()
            .filter(|r| r.r#type != butterflow_models::RuntimeType::Direct)
        {
            preamble.push_str(&self.warn(format!(
                "node '{}': runtime '{:?}' is not supported; its steps run directly",
                node.id, runtime.r#type
            )));
        }
        let description = node
            .description
            .as_deref()
            .map(|d| format!(": {d}"))
            .unwrap_or_default();
        let _ = writeln!(
            preamble,
            "/** {} */",
            comment_text(&format!("{}{description}", node.name))
        );

        let manual = node.r#type == NodeType::Manual
            || node
                .trigger
                .as_ref()
                .is_some_and(|t| t.r#type == TriggerType::Manual);
        let matrix_source =
            node.strategy
                .as_ref()
                .map(|strategy| match (&strategy.values, &strategy.from_state) {
                    (Some(values), _) => {
                        json_literal(&serde_json::to_value(values).unwrap_or_default())
                    }
                    (None, Some(key)) => format!("(state[{}] ?? [])", js_string(key)),
                    (None, None) => "[]".into(),
                });
        let meta_files = node
            .strategy
            .as_ref()
            .is_some_and(|s| s.from_state.is_some());

        let mut inner = String::new();
        if manual {
            self.import("approval");
            let _ = writeln!(
                inner,
                "  await step(approval({}));",
                js_string(&format!("Run '{}'?", node.name))
            );
        }

        let pull_request = node.pull_request.as_ref();
        let managed_git = node.branch_name.is_some() || pull_request.is_some();
        if managed_git {
            self.import("branch");
            let branch_expr = match &node.branch_name {
                Some(name) => template(name),
                None if matrix_source.is_some() => format!("`codemod-{}-${{index}}`", node.id),
                None => js_string(&format!("codemod-{}", node.id)),
            };
            let _ = writeln!(inner, "  const branchName = {branch_expr};");
            inner.push_str("  await step(branch(branchName));\n");
        }

        let steps = self.steps(
            &node.steps,
            &Scope {
                env: &node.env,
                meta_files,
                matrix: matrix_source.is_some(),
            },
            &templates,
        );
        inner.push_str(&steps);

        if let Some(pr) = pull_request {
            self.import("commit");
            self.import("pullRequest");
            let title = template(&pr.title);
            let _ = writeln!(inner, "  await step(commit({title}));");
            let mut fields = vec![("title", title)];
            if let Some(body) = &pr.body {
                fields.push(("body", template(body)));
            }
            if let Some(draft) = pr.draft {
                fields.push(("draft", draft.to_string()));
            }
            if let Some(base) = &pr.base {
                fields.push(("base", template(base)));
            }
            fields.push(("branch", "branchName".into()));
            let _ = writeln!(inner, "  await step(pullRequest({}));", object(&fields));
        }

        // What the node uses, its selectors included.
        let used = self.with_selectors(&inner);
        if matrix_source.is_none() && mentions(&used, "matrix") {
            inner.insert_str(0, "  const matrix: Matrix = null;\n");
        }
        NodeCode {
            function,
            preamble,
            uses_params: mentions(&used, "params"),
            uses_target: uses_target(&used),
            uses_state: mentions(&used, "state")
                || matrix_source
                    .as_deref()
                    .is_some_and(|source| mentions(source, "state")),
            inner,
            matrix: matrix_source,
            uses_git: false,
        }
    }

    fn steps(
        &mut self,
        steps: &[Step],
        scope: &Scope<'_>,
        templates: &HashMap<String, String>,
    ) -> String {
        let mut out = String::new();
        for step in steps {
            let (notes, statement) = self.step(step, scope, templates);
            let mut lines = notes;
            match &step.condition {
                Some(when) => {
                    let _ = write!(
                        lines,
                        "if ({}) {{\n{}",
                        condition(when),
                        indent(&statement, "  ")
                    );
                    if let Some(commit) = &step.commit {
                        let _ = writeln!(lines, "  {}", self.commit(commit));
                    }
                    lines.push_str("}\n");
                }
                None => {
                    lines.push_str(&statement);
                    lines.push('\n');
                    if let Some(commit) = &step.commit {
                        let _ = writeln!(lines, "{}", self.commit(commit));
                    }
                }
            }
            for line in lines.lines() {
                let _ = writeln!(out, "  {line}");
            }
        }
        out
    }

    fn commit(&mut self, commit: &butterflow_models::CommitConfig) -> String {
        self.import("commit");
        let mut fields = Vec::new();
        if let Some(add) = &commit.add {
            fields.push(("add", string_list(add)));
        }
        if commit.allow_empty {
            fields.push(("allowEmpty", "true".into()));
        }
        if fields.is_empty() {
            format!("await step(commit({}));", template(&commit.message))
        } else {
            format!(
                "await step(commit({}, {}));",
                template(&commit.message),
                object(&fields)
            )
        }
    }

    /// Warning comments and the statement for one step.
    fn step(
        &mut self,
        step: &Step,
        scope: &Scope<'_>,
        _templates: &HashMap<String, String>,
    ) -> (String, String) {
        // `${{ }}` in a step name is filled in, so matrix steps get distinct labels.
        let name = template(&step.name);
        let mut notes = String::new();
        let task = match &step.action {
            StepAction::RunScript(command) => {
                self.import("shell");
                let literal_echo = is_literal_echo(command);
                let mut env: Vec<(String, String)> = scope
                    .env
                    .iter()
                    .chain(step.env.iter().flatten())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<std::collections::BTreeMap<_, _>>()
                    .into_iter()
                    .collect();
                env.sort();
                let mut entries: Vec<String> = Vec::new();
                if self.plumb_state && !literal_echo {
                    self.uses_state_env = true;
                    entries.push("...stateEnv(state)".into());
                }
                if scope.matrix {
                    self.uses_matrix_env = true;
                    entries.push("...matrixEnv(matrix)".into());
                }
                entries.extend(
                    env.iter()
                        .map(|(k, v)| format!("{}: {}", property(k), js_string(v))),
                );
                // External scripts can write state without referencing it in YAML.
                // Only omit the wrapper for commands we know cannot write state.
                let capture_outputs = !literal_echo
                    && (self.plumb_state || command.to_ascii_uppercase().contains("STATE_OUTPUTS"));
                let command = if capture_outputs {
                    self.uses_posix_state_outputs = true;
                    template(&with_state_outputs(command))
                } else {
                    template(command)
                };
                let task = if entries.is_empty() {
                    format!("shell({command})")
                } else {
                    format!("shell({command}, {{ env: {{ {} }} }})", entries.join(", "))
                };
                if capture_outputs && self.plumb_state {
                    self.uses_state_outputs = true;
                    return (
                        notes,
                        format!("applyStateOutputs(state, await step({name}, {task}));"),
                    );
                }
                task
            }
            StepAction::AstGrep(ast_grep) => {
                self.import("forEachFile");
                self.import("astGrep");
                if ast_grep.allow_dirty.is_some() {
                    notes.push_str(&self.warn(format!(
                        "step '{}': allow_dirty had no effect and is dropped",
                        step.name
                    )));
                }
                if ast_grep.max_threads.is_some() {
                    notes.push_str(&self.warn(format!(
                        "step '{}': max_threads is dropped; the engine picks the number of threads",
                        step.name
                    )));
                }
                // The YAML engine matched the globs under `base_path`;
                // forEachFile matches them under the target.
                let base = ast_grep.base_path.as_deref().filter(|b| !is_root(b));
                if let Some(base) = base {
                    // A templated glob may stand for several, which the
                    // base is not joined to.
                    let templated = ast_grep
                        .include
                        .iter()
                        .chain(&ast_grep.exclude)
                        .flatten()
                        .any(|glob| glob.contains("${{"));
                    let outside = base.trim().starts_with('/')
                        || base.split(['/', '\\']).any(|part| part == "..");
                    if templated || outside {
                        notes.push_str(&self.warn(format!(
                            "step '{}': base_path '{base}' was joined to the include and exclude globs; check that they still select the same files",
                            step.name
                        )));
                    }
                }
                let rebased = |globs: &[String]| match base {
                    Some(base) => globs.iter().map(|glob| rebase_glob(base, glob)).collect(),
                    None => globs.to_vec(),
                };
                let mut selector = Vec::new();
                match &ast_grep.include {
                    Some(include) => selector.push(("include", string_list(&rebased(include)))),
                    None if scope.meta_files => {
                        selector.push(("include", "matrix?._meta_files".into()))
                    }
                    None => {
                        if let Some(base) = base {
                            selector.push(("include", string_list(&[rebase_glob(base, "**")])));
                        }
                    }
                }
                if let Some(exclude) = &ast_grep.exclude {
                    selector.push(("exclude", string_list(&rebased(exclude))));
                }
                let rules = format!(
                    "astGrep({})",
                    self.transform(&ast_grep.config_file, "Rules")
                );
                self.for_each_file(&selector, rules)
            }
            StepAction::JSAstGrep(js) => {
                self.import("forEachFile");
                if self.plumb_state && !self.v1_state {
                    notes.push_str(&self.warn(format!(
                        "step '{}': a transform can no longer read or set workflow state; if its script calls getState or setState, give it values through `params` and return what it finds as `data`",
                        step.name
                    )));
                }
                // Which files, in which language, with which semantic analysis
                // and capabilities, is the selector's part; the rest the callback's.
                let (selector, mut options) = self.jssg_selector(js, scope);
                if self.v1_state {
                    options.push(("v1State", "state".into()));
                }
                let callback = match self.plain_module(js) {
                    Ok(module) => self.jssg_plain(js, scope, &module),
                    Err(reason) => {
                        if let Some(reason) = reason {
                            if self.adapter_warned.insert(js.js_file.clone()) {
                                notes.push_str(&self.warn(format!(
                                    "{} runs through jssgFile, which runs a jssg transform as workflow.yaml did: {reason}",
                                    js.js_file
                                )));
                            }
                        }
                        self.jssg_adapter(js, options, "jssgFile")
                    }
                };
                let task = self.for_each_file(&selector, callback);
                if self.v1_state || self.v1_metrics {
                    let state = if self.v1_state { ", state" } else { "" };
                    return (
                        notes,
                        format!("v1Apply(await step({name}, {task}){state});"),
                    );
                }
                task
            }
            StepAction::Codemod(codemod) => {
                self.import("codemod");
                if codemod.args.is_some() || codemod.env.is_some() || codemod.working_dir.is_some()
                {
                    notes.push_str(&self.warn(format!(
                        "step '{}': args, env and working_dir had no effect and are dropped",
                        step.name
                    )));
                }
                if self.pass_params {
                    format!("codemod({}, {{ params }})", js_string(&codemod.source))
                } else {
                    format!("codemod({})", js_string(&codemod.source))
                }
            }
            StepAction::AI(ai) => {
                self.import("agent");
                if ai.working_dir.is_some()
                    || ai.env.is_some()
                    || ai.dry_run.is_some()
                    || ai.timeout_ms.is_some()
                    || ai.enable_lakeview.is_some()
                {
                    notes.push_str(&self.warn(format!(
                        "step '{}': working_dir, env, dry_run, timeout_ms and enable_lakeview had no effect and are dropped",
                        step.name
                    )));
                }
                let mut fields = Vec::new();
                if let Some(v) = &ai.model {
                    fields.push(("model", js_string(v)));
                }
                if let Some(v) = &ai.system_prompt {
                    fields.push(("systemPrompt", template(v)));
                }
                if let Some(v) = ai.max_steps {
                    fields.push(("maxSteps", v.to_string()));
                }
                if let Some(v) = &ai.tools {
                    fields.push(("tools", serde_json::to_string(v).unwrap_or_default()));
                }
                if let Some(v) = &ai.endpoint {
                    fields.push(("endpoint", js_string(v)));
                }
                if let Some(v) = &ai.api_key {
                    fields.push(("apiKey", js_string(v)));
                }
                if let Some(v) = &ai.llm_protocol {
                    fields.push(("llmProtocol", js_string(v)));
                }
                with_options(format!("agent({}", template(&ai.prompt)), &fields)
            }
            StepAction::InstallSkill(skill) => {
                self.import("installSkill");
                let value = serde_json::to_value(skill).unwrap_or(Value::Null);
                format!("installSkill({})", compact_object(&value))
            }
            StepAction::Shard(shard) => {
                self.import("shard");
                let mut fields = Vec::new();
                fields.push((
                    "method",
                    match &shard.method {
                        ShardMethod::Builtin(builtin) => {
                            let mut method = vec![(
                                "type",
                                js_string(match builtin.r#type {
                                    butterflow_models::step::BuiltinShardType::Directory => {
                                        "directory"
                                    }
                                    butterflow_models::step::BuiltinShardType::Codeowner => {
                                        "codeowner"
                                    }
                                }),
                            )];
                            method.push((
                                "maxFilesPerShard",
                                shard_count(&builtin.max_files_per_shard),
                            ));
                            if let Some(min) = &builtin.min_shard_size {
                                method.push(("minShardSize", min_shard_size(min)));
                            }
                            object(&method)
                        }
                        ShardMethod::Function(function) => {
                            let function = self.transform(&function.function, "Shards");
                            object(&[("function", function)])
                        }
                    },
                ));
                if let Some(target) = &shard.target {
                    fields.push(("target", template(target)));
                }
                match &shard.js_ast_grep {
                    // The files the transform would change, as the YAML engine's
                    // dry run of it found them.
                    Some(js) => {
                        let (selector, mut options) = self.jssg_selector(
                            js,
                            &Scope {
                                env: scope.env,
                                meta_files: false,
                                matrix: false,
                            },
                        );
                        if let Some(language) = selector.iter().find(|(key, _)| *key == "language")
                        {
                            options.insert(0, language.clone());
                        }
                        let filter = self.jssg_adapter(js, options, "jssgWouldChange");
                        let selector: Vec<_> = selector
                            .into_iter()
                            .filter(|(key, _)| matches!(*key, "include" | "exclude" | "language"))
                            .collect();
                        fields.push(("files", object(&selector)));
                        fields.push(("filter", filter));
                    }
                    None => {
                        if let Some(pattern) = &shard.file_pattern {
                            fields.push(("files", js_string(pattern)));
                        }
                    }
                }
                let key = format!("state[{}]", js_string(&shard.output_state));
                fields.push(("previous", key.clone()));
                let statement = format!("{key} = await step({name}, shard({}));", object(&fields));
                return (notes, statement);
            }
            StepAction::UseTemplate(use_template) => {
                let function = self.function_name("Template", &use_template.template);
                let inputs = serde_json::to_value(&use_template.inputs)
                    .map(|inputs| json_literal(&inputs))
                    .unwrap_or_else(|_| "{}".into());
                let target = if self.templates_take_target {
                    ", target"
                } else {
                    ""
                };
                let statement = if use_template.inputs.is_empty() {
                    format!("await {function}(params, matrix, state{target});")
                } else {
                    format!(
                        "await {function}({{ ...params, ...{inputs} }}, matrix, state{target});"
                    )
                };
                return (notes, statement);
            }
        };
        (notes, format!("await step({name}, {task});"))
    }

    fn jssg_fields(&mut self, js: &UseJSAstGrep, scope: &Scope<'_>) -> Vec<(&'static str, String)> {
        let mut fields = Vec::new();
        match &js.include {
            Some(include) => fields.push(("include", string_list(include))),
            None if scope.meta_files => fields.push(("files", "matrix?._meta_files".into())),
            None => {}
        }
        if let Some(exclude) = &js.exclude {
            fields.push(("exclude", string_list(exclude)));
        }
        if let Some(base) = js.base_path.as_deref().filter(|b| !is_root(b)) {
            fields.push(("basePath", template(base)));
        }
        if let Some(language) = &js.language {
            fields.push(("language", js_string(language)));
        }
        if let Some(capabilities) = &js.capabilities {
            let capabilities: Vec<String> = serde_json::to_value(capabilities)
                .ok()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default();
            fields.push(("capabilities", string_list(&capabilities)));
        }
        if let Some(semantic) = &js.semantic_analysis {
            let mode = |mode: &SemanticAnalysisMode| match mode {
                SemanticAnalysisMode::File => "file",
                SemanticAnalysisMode::Workspace => "workspace",
            };
            fields.push((
                "semanticAnalysis",
                match semantic {
                    SemanticAnalysisConfig::Mode(m) => js_string(mode(m)),
                    SemanticAnalysisConfig::Detailed(detail) => {
                        let mut parts = vec![("mode", js_string(mode(&detail.mode)))];
                        if let Some(root) = &detail.root {
                            parts.push(("root", js_string(root)));
                        }
                        object(&parts)
                    }
                },
            ));
        }
        if let Some(threads) = js.max_threads {
            fields.push(("threads", threads.to_string()));
        }
        if js.dry_run == Some(true) {
            fields.push(("dryRun", "true".into()));
        }
        if self.pass_params {
            fields.push(("params", "params".into()));
        }
        if scope.matrix {
            fields.push(("matrix", "matrix ?? undefined".into()));
        }
        fields
    }

    fn finish(mut self) -> Translated {
        // Selectors in place first: what they use is imported like the rest.
        let mut consts = String::new();
        let body = self.body.clone();
        self.body = self.selectors_in(body, &mut consts);
        let mut source = String::new();
        source.push_str("// Generated from workflow.yaml by `codemod workflow upgrade`.\n");
        for warning in &self.warnings {
            let _ = writeln!(source, "// codemod upgrade: {}", comment_text(warning));
        }
        let (git, mut imports): (Vec<&str>, Vec<&str>) = self
            .imports
            .iter()
            .copied()
            .partition(|name| GIT_STEPS.contains(name));
        if self.body.contains("evaluate(") {
            imports.push("__evaluate as evaluate");
        }
        imports.sort_by_key(|name| name.trim_start_matches("__"));
        if self.uses_parse {
            source.push_str("import { parse } from \"codemod/ast-grep\";\n");
        }
        let _ = writeln!(
            source,
            "import {{ {} }} from \"codemod/workflow\";",
            imports.join(", ")
        );
        if !git.is_empty() {
            let _ = writeln!(
                source,
                "import {{ {} }} from \"codemod/git\";",
                git.join(", ")
            );
        }
        if self.uses_context {
            source.push_str("import type { WorkflowContext } from \"codemod/workflow\";\n");
        }
        // Shard functions run where callbacks run; rule files are data.
        for (name, specifier) in &self.transforms {
            let _ = writeln!(source, "import {name} from {};", js_string(specifier));
        }
        for (name, specifier, selector) in &self.plain_transforms {
            let named = match selector.as_deref() {
                None => String::new(),
                Some("getSelector") => ", { getSelector }".into(),
                Some(local) => format!(", {{ getSelector as {local} }}"),
            };
            let _ = writeln!(
                source,
                "import {name}{named} from {};",
                js_string(specifier)
            );
        }
        if !self.jssg_modules.is_empty() {
            for (name, specifier) in &self.jssg_modules {
                let _ = writeln!(source, "import * as {name} from {};", js_string(specifier));
            }
            let adapter: Vec<&str> = self.adapter.iter().copied().collect();
            let _ = writeln!(
                source,
                "import {{ {} }} from \"codemod-internal:jssg-compat\";",
                adapter.join(", ")
            );
        }
        source.push('\n');
        let mut aliases = String::new();
        if mentions(&self.body, "Params") {
            // What jssg types a transform's params as, when one gets them.
            let value = if self.params_to_transforms && self.string_defaults {
                "string"
            } else {
                "unknown"
            };
            let _ = writeln!(aliases, "type Params = Record<string, {value}>;");
        }
        if mentions(&self.body, "Matrix") || self.uses_matrix_env {
            aliases.push_str("type Matrix = Record<string, any> | null;\n");
        }
        if mentions(&self.body, "State") || self.uses_state_outputs || self.uses_state_env {
            // workflow.yaml declares no types for state, so it is read as `any`.
            aliases.push_str("type State = Record<string, any>;\n");
        }
        if !aliases.is_empty() {
            source.push_str(&aliases);
            source.push('\n');
        }
        source.push_str(&consts);
        source.push_str(&self.body);
        let mut source = print::format(&source);
        if self.uses_after {
            source.push_str(
                "\nfunction after(dependencies: Promise<unknown>[], run: () => Promise<unknown>) {\n  return Promise.all(dependencies).then(run);\n}\n",
            );
        }
        if self.uses_state_env {
            source.push_str(
                "\n/** State as `CODEMOD_STATE_<KEY>` variables, JSON-encoded as before. */\nfunction stateEnv(state: State): Record<string, string> {\n  return Object.fromEntries(\n    Object.entries(state).map(([key, value]) => [`CODEMOD_STATE_${key.toUpperCase()}`, JSON.stringify(value)]),\n  );\n}\n",
            );
        }
        if self.uses_state_outputs {
            source.push_str(&STATE_OUTPUTS_HELPER.replace("{marker}", STATE_MARKER));
        }
        if self.body.contains(&format!("{}(", expr::TEXT)) {
            source.push_str(expr::TEXT_HELPER);
        }
        if self.body.contains("optionalCount(") {
            source.push_str(
                "\n/** An optional count from a `${{ }}`: empty (a null value) leaves it unset. */\nfunction optionalCount(text: string): number | undefined {\n  return text.trim() === \"\" ? undefined : Number(text);\n}\n",
            );
        }
        if self.v1_state || self.v1_metrics {
            source.push_str(v1::HELPERS);
        }
        if self.uses_matrix_env {
            source.push_str(
                "\n/** Matrix values as environment variables, JSON-encoded as before. */\nfunction matrixEnv(matrix: Matrix): Record<string, string> {\n  return Object.fromEntries(\n    Object.entries(matrix ?? {}).map(([key, value]) => [key, JSON.stringify(value)]),\n  );\n}\n",
            );
        }
        Translated {
            source,
            warnings: self.warnings,
        }
    }
}

/// A literal `echo` is a builtin of both sh and cmd. Do not attempt to recognize
/// shell syntax here: variables, redirects, command separators, etc. can all
/// make a command stateful.
fn is_literal_echo(command: &str) -> bool {
    command.strip_prefix("echo ").is_some_and(|text| {
        !text.trim().is_empty()
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'.' | b'_' | b'-'))
    })
}

/// Marks where a translated step's `$STATE_OUTPUTS` lines start in its stdout.
const STATE_MARKER: &str = "::codemod-state-outputs::";

/// `command` with `$STATE_OUTPUTS` set to a file, as the YAML engine ran it.
/// What the command writes there is printed after [`STATE_MARKER`], last,
/// for `applyStateOutputs` to read; the exit code is the command's.
fn with_state_outputs(command: &str) -> String {
    let run = if command.starts_with("#!") {
        // A script naming its interpreter runs from a file, as it did. The
        // heredoc ends at the first line equal to its delimiter, so the
        // delimiter is one the script has no line of.
        let mut end = "CODEMOD_SCRIPT_END".to_string();
        while command.lines().any(|line| line == end) {
            end.push('_');
        }
        format!(
            "cat > \"$CODEMOD_STATE_DIR/script\" <<'{end}'\n{command}\n{end}\nchmod +x \"$CODEMOD_STATE_DIR/script\"\n\"$CODEMOD_STATE_DIR/script\""
        )
    } else {
        // A subshell, so an `exit` in the command still prints the lines.
        format!("(\n{command}\n)")
    };
    format!(
        "CODEMOD_STATE_DIR=\"$(mktemp -d)\" || exit 1\nexport STATE_OUTPUTS=\"$CODEMOD_STATE_DIR/outputs\"\n: > \"$STATE_OUTPUTS\"\n{run}\ncodemod_status=$?\nprintf '\\n%s\\n' '{STATE_MARKER}'\ncat \"$STATE_OUTPUTS\"\nrm -rf \"$CODEMOD_STATE_DIR\"\nexit $codemod_status"
    )
}

/// Reads the lines `with_state_outputs` printed into `state`, as the YAML
/// engine did: `key=value` sets, `key@=value` appends, a value is JSON or
/// else a string, and `key=null` removes the key.
const STATE_OUTPUTS_HELPER: &str = r#"
/** Copies what a step wrote to `$STATE_OUTPUTS` into `state`. */
function applyStateOutputs(state: State, result: { stdout?: string }): void {
  const stdout = result.stdout ?? "";
  const start = stdout.lastIndexOf("\n{marker}\n");
  if (start < 0) {
    // The wrapper always prints the marker; stdout keeps only its last 1 MiB.
    if (result.stdout === undefined) return;
    throw new Error("the step wrote more than 1 MiB to $STATE_OUTPUTS, so its state lines were cut");
  }
  for (const line of stdout.slice(start + "\n{marker}\n".length).split("\n")) {
    if (!line.trim()) continue;
    const equals = line.indexOf("=");
    const append = equals > 0 && line[equals - 1] === "@";
    const key = line.slice(0, append ? equals - 1 : Math.max(equals, 0));
    if (!key || key.trim() !== key) {
      console.warn(`skipped a $STATE_OUTPUTS line that is not key=value: ${line}`);
      continue;
    }
    const text = line.slice(equals + 1);
    let value: unknown;
    try {
      value = JSON.parse(text);
    } catch {
      value = text;
    }
    if (append) {
      const list = state[key] ?? [];
      state[key] = [...(Array.isArray(list) ? list : [list]), value];
    } else if (value === null) {
      delete state[key];
    } else {
      state[key] = value;
    }
  }
}
"#;

fn workflow_templates(_workflow: &Workflow) -> HashMap<String, String> {
    HashMap::new()
}

/// The js-ast-grep steps of a workflow, its templates' included.
fn jssg_steps(workflow: &Workflow) -> Vec<&UseJSAstGrep> {
    let mut found = Vec::new();
    for node in &workflow.nodes {
        collect_jssg(&node.steps, &mut found);
    }
    for template in &workflow.templates {
        collect_jssg(&template.steps, &mut found);
    }
    found
}

fn collect_jssg<'a>(steps: &'a [Step], found: &mut Vec<&'a UseJSAstGrep>) {
    for step in steps {
        match &step.action {
            StepAction::JSAstGrep(js) => found.push(js),
            StepAction::Shard(shard) => found.extend(shard.js_ast_grep.as_ref()),
            _ => {}
        }
    }
}

/// Nodes ordered so every node comes after its dependencies.
fn topological(nodes: &[Node]) -> Vec<&Node> {
    let mut ordered: Vec<&Node> = Vec::new();
    let mut placed: BTreeSet<&str> = BTreeSet::new();
    while ordered.len() < nodes.len() {
        let before = ordered.len();
        for node in nodes {
            if placed.contains(node.id.as_str()) {
                continue;
            }
            if node.depends_on.iter().all(|d| placed.contains(d.as_str())) {
                placed.insert(&node.id);
                ordered.push(node);
            }
        }
        if ordered.len() == before {
            // A cycle or a missing dependency; validation reports it.
            for node in nodes {
                if placed.insert(&node.id) {
                    ordered.push(node);
                }
            }
        }
    }
    ordered
}

fn property(key: &str) -> String {
    let identifier = key.chars().enumerate().all(|(i, c)| {
        c == '_' || c == '$' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
    });
    if identifier && !key.is_empty() {
        key.to_string()
    } else {
        js_string(key)
    }
}

/// Whether `text` uses `word` as an identifier. Text inside generated string
/// literals counts too, which only ever declares a name that goes unused.
fn mentions(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        let before = &text[..at];
        let after = text[at + word.len()..].chars().next();
        let boundary =
            |c: Option<char>| !c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
        // `a.word` is a property; `...word` spreads the identifier.
        let property = before.ends_with('.') && !before.ends_with("...");
        boundary(before.chars().next_back()) && boundary(after) && !property
    })
}

/// A YAML `base_path` that means the target itself.
fn is_root(base: &str) -> bool {
    matches!(base.trim(), "" | "." | "./")
}

/// `glob`, which the YAML engine matched under `base`, as a glob under the
/// target. As in `.gitignore`, a glob with no slash (but a trailing one)
/// matches at any depth, and one with a slash is anchored at the base.
fn rebase_glob(base: &str, glob: &str) -> String {
    let base = base.trim();
    let base = base
        .strip_prefix("./")
        .unwrap_or(base)
        .trim_end_matches('/');
    let (negated, glob) = match glob.strip_prefix('!') {
        Some(rest) => ("!", rest),
        None => ("", glob),
    };
    let anchored = glob.trim_end_matches('/').contains('/');
    let glob = glob
        .strip_prefix("./")
        .unwrap_or(glob)
        .trim_start_matches('/');
    if anchored || glob.starts_with("**") {
        format!("{negated}{base}/{glob}")
    } else {
        format!("{negated}{base}/**/{glob}")
    }
}

/// Prefix every line with `prefix`.
fn indent(text: &str, prefix: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            let _ = writeln!(out, "{prefix}{line}");
        }
    }
    out
}

fn object(fields: &[(&str, String)]) -> String {
    if fields.is_empty() {
        return "{}".into();
    }
    let parts: Vec<String> = fields
        .iter()
        .map(|(key, value)| {
            if key == value {
                key.to_string()
            } else {
                format!("{}: {value}", property(key))
            }
        })
        .collect();
    format!("{{ {} }}", parts.join(", "))
}

/// `call` (an open call such as `jssg(transform`) closed with its options.
fn with_options(call: String, fields: &[(&str, String)]) -> String {
    if fields.is_empty() {
        return format!("{call})");
    }
    format!("{call}, {})", object(fields))
}

/// Marks where a `forEachFile` selector goes, until `finish` knows whether
/// it is shared.
const SELECTOR_MARK: &str = "\u{1}selector";

/// Whether a selector is made of literals only, so it can be a const.
fn is_literal_object(text: &str) -> bool {
    let Some(inner) = text.strip_prefix("{ ").and_then(|t| t.strip_suffix(" }")) else {
        return false;
    };
    // Each `key: value` value is JSON; keys are plain identifiers.
    let json = format!(
        "{{{}}}",
        inner
            .split(", ")
            .fold((String::new(), 0i32), |(mut out, depth), part| {
                let opening = part.matches(['[', '{']).count() as i32;
                let closing = part.matches([']', '}']).count() as i32;
                if !out.is_empty() {
                    out.push_str(", ");
                }
                match (depth, part.split_once(": ")) {
                    (0, Some((key, value))) if key.chars().all(|c| c.is_ascii_alphanumeric()) => {
                        let _ = write!(out, "\"{key}\": {value}");
                    }
                    _ => out.push_str(part),
                }
                (out, depth + opening - closing)
            })
            .0
    );
    serde_json::from_str::<Value>(&json).is_ok()
}

/// The `language` a selector names.
fn language_of(text: &str) -> Option<String> {
    let rest = text.split("language: \"").nth(1)?;
    let language = rest.split('"').next()?;
    Some(
        language
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase(),
    )
}

/// A JSON value as TypeScript, with spaces after commas and colons.
fn json_literal(value: &Value) -> String {
    match value {
        Value::Object(map) if map.is_empty() => "{}".into(),
        Value::Object(map) => {
            let fields: Vec<String> = map
                .iter()
                .map(|(key, value)| format!("{}: {}", property(key), json_literal(value)))
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(json_literal)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => other.to_string(),
    }
}

/// A JSON value as a compact object literal, dropping nulls.
fn compact_object(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let fields: Vec<(&str, String)> = map
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.as_str(), compact_object(v)))
                .collect();
            object(&fields)
        }
        other => other.to_string(),
    }
}

/// An optional minimum shard size. A `${{ }}` that prints nothing (a null
/// value) leaves it unset, as a null minimum did before.
fn min_shard_size(value: &Value) -> String {
    match value {
        Value::String(text) if text.contains("${{") => {
            format!("optionalCount({})", template(text))
        }
        other => other.to_string(),
    }
}

/// A shard size: a number, or a `${{ }}` expression evaluated at run time.
fn shard_count(value: &Value) -> String {
    match value {
        Value::String(text) if text.contains("${{") => {
            format!("Number({})", template(text))
        }
        other => other.to_string(),
    }
}

/// Text that is safe inside a `/* */` or `//` comment: on one line, and
/// unable to close the comment (`api/**/*.ts` holds a `*/`).
fn comment_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("*/", "*\\/")
}

#[cfg(test)]
mod tests;

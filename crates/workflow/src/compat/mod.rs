//! workflow.yaml compatibility: translates a YAML workflow into the
//! equivalent TypeScript workflow. `workflow run` uses this in memory;
//! `codemod workflow upgrade` writes the result next to the YAML.
//!
//! - a workflow with one plain node becomes a `main` that runs its steps;
//!   otherwise each node becomes an async function, run in order when the
//!   nodes form a chain, together when they are independent, and through a
//!   promise graph for anything else, so independent nodes still run at the
//!   same time;
//! - each step becomes `await step("<name>", <task>)`;
//! - matrix nodes run once per value (`values`, or the array in state for
//!   `from_state`), concurrently;
//! - `${{ }}` expressions become TypeScript where that is exact, and
//!   otherwise call `evaluate(...)`, which uses the YAML engine's evaluator;
//! - `trigger: manual` becomes an `approval` step; `branch_name`,
//!   `pull_request` and step `commit` become git steps.

pub mod expr;

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::Path;

use butterflow_models::node::NodeType;
use butterflow_models::step::{
    SemanticAnalysisConfig, SemanticAnalysisMode, ShardMethod, StepAction, UseJSAstGrep,
};
use butterflow_models::trigger::TriggerType;
use butterflow_models::{Node, Step, Workflow};
use serde_json::Value;

use expr::{condition, js_string, string_list, template};

/// A translated workflow.
#[derive(Debug, Clone)]
pub struct Translated {
    pub source: String,
    /// Features that have no effect after translation.
    pub warnings: Vec<String>,
}

/// Parse, validate and translate a workflow.yaml (or JSON) file.
pub fn translate_file(path: &Path) -> Result<Translated, String> {
    let workflow = butterflow_core::utils::parse_workflow_file(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let bundle = path.parent().unwrap_or(Path::new("."));
    butterflow_core::utils::validate_workflow(&workflow, bundle)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(translate(&workflow))
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
    /// Tasks get the run's params anyway; pass `params` explicitly only when
    /// it differs from them (defaults, template inputs).
    pass_params: bool,
    names: HashMap<String, String>,
    taken: BTreeSet<String>,
}

/// One node, generated before deciding how `main` runs it.
struct NodeCode {
    function: String,
    /// Doc comment and warnings, without indentation.
    preamble: String,
    /// The node's statements, indented for a function body.
    inner: String,
    /// Matrix values (`values` or state) when the node runs once per value.
    matrix: Option<String>,
    uses_params: bool,
}

/// Where a list of steps is being generated.
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

    fn warn(&mut self, message: String) -> String {
        self.warnings.push(message.clone());
        format!("// codemod upgrade: {message}\n")
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

        let order = topological(&workflow.nodes);
        let nodes: Vec<NodeCode> = order.iter().map(|node| self.node_code(node)).collect();

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
            let _ = writeln!(
                templates_text,
                "/** {} */\nasync function {name}(params: Params, matrix: Matrix) {{",
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
        let mut main = String::new();
        if has_defaults {
            self.uses_context = true;
            main.push_str("export default async function main({ params: input }: WorkflowContext<Params>) {\n");
            main.push_str("  const params: Params = { ...DEFAULT_PARAMS, ...input };\n");
        } else if uses_params {
            self.uses_context = true;
            main.push_str(
                "export default async function main({ params }: WorkflowContext<Params>) {\n",
            );
        } else {
            main.push_str("export default async function main() {\n");
        }

        let call = |node: &NodeCode| {
            if node.uses_params {
                format!("{}(params)", node.function)
            } else {
                format!("{}()", node.function)
            }
        };
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
        main.push_str("}\n");
        self.body.push_str(&main);

        if !single {
            for code in &nodes {
                self.body.push('\n');
                self.body.push_str(&code.preamble);
                let (params, pass) = if code.uses_params {
                    ("params: Params", "params, ")
                } else {
                    ("", "")
                };
                match &code.matrix {
                    Some(source) => {
                        let instance = format!("{}For", code.function);
                        let _ = writeln!(
                            self.body,
                            "async function {}({params}) {{\n  await Promise.all(\n    {source}.map((matrix: Matrix, index: number) => {instance}({pass}matrix, index)),\n  );\n}}\n",
                            code.function
                        );
                        let _ = writeln!(
                            self.body,
                            "async function {instance}({pass}matrix: Matrix, index: number) {{",
                            pass = if code.uses_params {
                                "params: Params, "
                            } else {
                                ""
                            }
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
                serde_json::to_string_pretty(&defaults).unwrap_or_else(|_| "{}".into())
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
        let _ = writeln!(preamble, "/** {}{description} */", node.name);

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
                        serde_json::to_string(values).unwrap_or_else(|_| "[]".into())
                    }
                    (None, Some(key)) => {
                        self.import("getState");
                        format!("(getState<any>({}) ?? [])", js_string(key))
                    }
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

        if matrix_source.is_none() && mentions(&inner, "matrix") {
            inner.insert_str(0, "  const matrix: Matrix = null;\n");
        }
        NodeCode {
            function,
            preamble,
            uses_params: mentions(&inner, "params"),
            inner,
            matrix: matrix_source,
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
                if scope.matrix {
                    self.uses_matrix_env = true;
                    entries.push("...matrixEnv(matrix)".into());
                }
                entries.extend(
                    env.iter()
                        .map(|(k, v)| format!("{}: {}", property(k), js_string(v))),
                );
                if entries.is_empty() {
                    format!("shell({})", template(command))
                } else {
                    format!(
                        "shell({}, {{ env: {{ {} }} }})",
                        template(command),
                        entries.join(", ")
                    )
                }
            }
            StepAction::AstGrep(ast_grep) => {
                self.import("astGrep");
                if ast_grep.allow_dirty.is_some() {
                    notes.push_str(&self.warn(format!(
                        "step '{}': allow_dirty had no effect and is dropped",
                        step.name
                    )));
                }
                let mut fields = Vec::new();
                match &ast_grep.include {
                    Some(include) => fields.push(("include", string_list(include))),
                    None if scope.meta_files => {
                        fields.push(("include", "matrix?._meta_files".into()))
                    }
                    None => {}
                }
                if let Some(exclude) = &ast_grep.exclude {
                    fields.push(("exclude", string_list(exclude)));
                }
                if let Some(base) = ast_grep.base_path.as_deref().filter(|b| !is_root(b)) {
                    fields.push(("basePath", js_string(base)));
                }
                if let Some(threads) = ast_grep.max_threads {
                    fields.push(("threads", threads.to_string()));
                }
                with_options(
                    format!("astGrep({}", js_string(&ast_grep.config_file)),
                    &fields,
                )
            }
            StepAction::JSAstGrep(js) => {
                self.import("jssg");
                let fields = self.jssg_fields(js, scope);
                with_options(format!("jssg({}", js_string(&js.js_file)), &fields)
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
                self.import("setState");
                self.import("getState");
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
                                method.push(("minShardSize", shard_count(min)));
                            }
                            object(&method)
                        }
                        ShardMethod::Function(function) => {
                            object(&[("function", js_string(&function.function))])
                        }
                    },
                ));
                if let Some(target) = &shard.target {
                    fields.push(("target", template(target)));
                }
                if let Some(pattern) = &shard.file_pattern {
                    fields.push(("filePattern", js_string(pattern)));
                }
                if let Some(js) = &shard.js_ast_grep {
                    let mut js_fields = vec![("path", js_string(&js.js_file))];
                    js_fields.extend(self.jssg_fields(
                        js,
                        &Scope {
                            env: scope.env,
                            meta_files: false,
                            matrix: false,
                        },
                    ));
                    fields.push(("jssg", object(&js_fields)));
                }
                fields.push((
                    "previous",
                    format!("getState<any>({})", js_string(&shard.output_state)),
                ));
                let statement = format!(
                    "setState({}, await step({name}, shard({})));",
                    js_string(&shard.output_state),
                    object(&fields)
                );
                return (notes, statement);
            }
            StepAction::UseTemplate(use_template) => {
                let function = self.function_name("Template", &use_template.template);
                let inputs =
                    serde_json::to_string(&use_template.inputs).unwrap_or_else(|_| "{}".into());
                let statement = if use_template.inputs.is_empty() {
                    format!("await {function}(params, matrix);")
                } else {
                    format!("await {function}({{ ...params, ...{inputs} }}, matrix);")
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
            fields.push((
                "capabilities",
                serde_json::to_string(capabilities).unwrap_or_default(),
            ));
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

    fn finish(self) -> Translated {
        let mut source = String::new();
        source.push_str("// Generated from workflow.yaml by `codemod workflow upgrade`.\n");
        source.push_str("/// <reference types=\"codemod/workflow\" />\n");
        for warning in &self.warnings {
            let _ = writeln!(source, "// codemod upgrade: {warning}");
        }
        let mut imports: Vec<&str> = self.imports.iter().copied().collect();
        if self.body.contains("evaluate(") {
            imports.push("__evaluate as evaluate");
        }
        if (self.body.contains("getState(") || self.body.contains("getState<"))
            && !imports.contains(&"getState")
        {
            imports.push("getState");
        }
        imports.sort_by_key(|name| name.trim_start_matches("__"));
        let _ = writeln!(
            source,
            "import {{ {} }} from \"codemod:workflow\";",
            imports.join(", ")
        );
        if self.uses_context {
            source.push_str("import type { WorkflowContext } from \"codemod:workflow\";\n");
        }
        source.push('\n');
        let mut aliases = String::new();
        if mentions(&self.body, "Params") {
            aliases.push_str("type Params = Record<string, any>;\n");
        }
        if mentions(&self.body, "Matrix") || self.uses_matrix_env {
            aliases.push_str("type Matrix = Record<string, any> | null;\n");
        }
        if !aliases.is_empty() {
            source.push_str(&aliases);
            source.push('\n');
        }
        source.push_str(&self.body);
        if self.uses_after {
            source.push_str(
                "\nfunction after(dependencies: Promise<unknown>[], run: () => Promise<unknown>) {\n  return Promise.all(dependencies).then(run);\n}\n",
            );
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

fn workflow_templates(_workflow: &Workflow) -> HashMap<String, String> {
    HashMap::new()
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

/// Whether `text` uses `word` as an identifier.
fn mentions(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        let boundary =
            |c: Option<char>| !c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
        boundary(before) && boundary(after) && before != Some('.')
    })
}

/// A YAML `base_path` that means the target itself.
fn is_root(base: &str) -> bool {
    matches!(base.trim(), "" | "." | "./")
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

/// `call` (an open call such as `jssg("a.ts"`) closed with its options.
/// Long option objects go on several lines.
fn with_options(call: String, fields: &[(&str, String)]) -> String {
    if fields.is_empty() {
        return format!("{call})");
    }
    let one_line = format!("{call}, {})", object(fields));
    if one_line.len() <= 80 {
        return one_line;
    }
    let mut out = format!("{call}, {{\n");
    for (key, value) in fields {
        if key == value {
            let _ = writeln!(out, "  {key},");
        } else {
            let _ = writeln!(out, "  {}: {value},", property(key));
        }
    }
    out.push_str("})");
    out
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

/// A shard size: a number, or a `${{ }}` expression evaluated at run time.
fn shard_count(value: &Value) -> String {
    match value {
        Value::String(text) if text.contains("${{") => {
            format!("Number({})", template(text))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;

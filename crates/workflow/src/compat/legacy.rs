//! What `codemod workflow upgrade` changes in a codemod 1.x package besides
//! its workflow file, and what stops it.
//!
//! - [`rewrite`] changes what 2.0 removed and can be done another way:
//!   `codemod:metrics` imports point at a small module it adds to the
//!   package ([`SUPPORT_DIR`]), and the transform is called through a
//!   wrapper that returns the counts as the file's data; a transform that
//!   imports `fs` or `child_process` asks for that capability, where 1.x
//!   gave transforms a file system without asking;
//! - what cannot run on 2.0 is an upgrade error that names it: workflow
//!   state in a transform ([`rewrite`]) and containers
//!   ([`container_runtimes`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

use butterflow_models::step::{StepAction, UseJSAstGrep};
use butterflow_models::{RuntimeType, Workflow};

use super::transform_module::{self, Analysis, TransformModule};

/// Where the upgrade puts the files it adds to a package.
pub const SUPPORT_DIR: &str = "__codemod_v1";

/// `codemod:workflow` exports that 1.x transforms could call and 2.0 removed.
const STATE_API: &[&str] = &[
    "getState",
    "setState",
    "unsetState",
    "acquireLock",
    "setStepOutput",
    "getStepOutput",
    "getOrSetStepOutput",
];

/// What upgrading changes besides the workflow file.
#[derive(Debug, Default)]
pub(crate) struct Rewrites {
    /// Files to write: added ones, and transforms with rewritten imports.
    pub files: Vec<(PathBuf, String)>,
    /// How the generated workflow calls the wrappers, by `js_file`.
    pub analyses: HashMap<String, Analysis>,
    /// What changed, for the person upgrading.
    pub notes: Vec<String>,
    /// Capabilities transforms now ask for.
    pub capabilities: BTreeSet<String>,
}

/// Rewrite what 2.0 removed in the transforms `workflow` runs. `bundle` is
/// the workflow's directory; `js_file`s are changed in `workflow` to point
/// at wrappers. Returns the problems that cannot be rewritten.
pub(crate) fn rewrite(workflow: &mut Workflow, bundle: &Path) -> Result<Rewrites, Vec<String>> {
    let bundle = normalize(bundle);
    let mut out = Rewrites::default();
    let mut problems = Vec::new();
    let mut reported = BTreeSet::new();
    let mut metric_files = BTreeSet::new();
    let mut wrappers: BTreeMap<String, String> = BTreeMap::new();
    let mut asked: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
    for js in jssg_steps_mut(workflow) {
        let entry = normalize(&bundle.join(&js.js_file));
        let mut uses_metrics = false;
        let mut capabilities = BTreeSet::new();
        for file in closure(&entry) {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let shown = shown(&bundle, &file);
            let state = state_calls(&text);
            if !state.is_empty() && reported.insert(shown.clone()) {
                problems.push(format!(
                    "{shown} calls {} from codemod:workflow. 2.0 has no workflow state, so this transform cannot run as written: it has to return what it found (`{{ edit, data }}`) for the workflow to read from the step's result",
                    state.join(", ")
                ));
            }
            if imports_metrics(&text) {
                uses_metrics = true;
                metric_files.insert(file.clone());
            }
            for specifier in import_specifiers(&file, &text) {
                if let Some(capability) = capability_of(&specifier) {
                    capabilities.insert(capability);
                }
            }
        }
        if !capabilities.is_empty() {
            let mut list = js.capabilities.clone().unwrap_or_default();
            for capability in &capabilities {
                if !list.iter().any(|c| c == capability) {
                    list.push(capability.to_string());
                    asked
                        .entry(js.js_file.clone())
                        .or_default()
                        .insert(capability);
                }
            }
            out.capabilities
                .extend(capabilities.iter().map(|c| c.to_string()));
            js.capabilities = Some(list);
        }
        if !uses_metrics {
            continue;
        }
        if let Some(wrapper) = wrappers.get(&js.js_file) {
            js.js_file = wrapper.clone();
            continue;
        }
        let Some(relative) = relative_to(&bundle, &entry) else {
            problems.push(format!(
                "{} uses codemod:metrics and is outside the package",
                entry.display()
            ));
            continue;
        };
        let stem = relative
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or(&relative)
            .replace('/', "__");
        let wrapper = format!("{SUPPORT_DIR}/{stem}.ts");
        let analysis = transform_module::analyze(&entry);
        let get_selector = matches!(&analysis, Analysis::Plain(module) if module.get_selector);
        out.files.push((
            bundle.join(&wrapper),
            wrapper_source(&relative, get_selector),
        ));
        out.analyses.insert(
            wrapper.clone(),
            match analysis {
                Analysis::Plain(module) => Analysis::Plain(TransformModule {
                    takes_options: true,
                    is_async: true,
                    returns_result: true,
                    get_selector: module.get_selector,
                }),
                adapter => adapter,
            },
        );
        wrappers.insert(js.js_file.clone(), wrapper.clone());
        js.js_file = wrapper;
    }
    for (file, capabilities) in &asked {
        let names: Vec<&str> = capabilities.iter().copied().collect();
        out.notes.push(format!(
            "{file} imports {}: its step now asks for the {} capabilit{} (1.x gave transforms these without asking)",
            names.join(", "),
            names.join(", "),
            if names.len() == 1 { "y" } else { "ies" }
        ));
    }
    if !metric_files.is_empty() {
        out.files.push((
            bundle.join(SUPPORT_DIR).join("metrics.ts"),
            METRICS_MODULE.to_string(),
        ));
        for file in &metric_files {
            let Some(relative) = relative_to(&bundle, file) else {
                problems.push(format!(
                    "{} uses codemod:metrics and is outside the package",
                    file.display()
                ));
                continue;
            };
            let Ok(text) = std::fs::read_to_string(file) else {
                continue;
            };
            let depth = relative.matches('/').count();
            let specifier = if depth == 0 {
                format!("./{SUPPORT_DIR}/metrics.ts")
            } else {
                format!("{}{SUPPORT_DIR}/metrics.ts", "../".repeat(depth))
            };
            let text = text
                .replace("\"codemod:metrics\"", &format!("\"{specifier}\""))
                .replace("'codemod:metrics'", &format!("'{specifier}'"));
            out.files.push((file.clone(), text));
        }
        let names: Vec<String> = metric_files.iter().map(|f| shown(&bundle, f)).collect();
        out.notes.push(format!(
            "{} used codemod:metrics, which 2.0 removed: {} now import{} {SUPPORT_DIR}/metrics.ts, and the counts are each file's data in the step's result",
            names.join(", "),
            if names.len() == 1 { "it" } else { "they" },
            if names.len() == 1 { "s" } else { "" },
        ));
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

/// Nodes and templates that run in a container: 2.0 runs none.
pub(crate) fn container_runtimes(workflow: &Workflow) -> Vec<String> {
    let nodes = workflow
        .nodes
        .iter()
        .map(|node| ("node", &node.id, &node.runtime));
    let templates = workflow
        .templates
        .iter()
        .map(|template| ("template", &template.id, &template.runtime));
    nodes
        .chain(templates)
        .filter_map(|(kind, id, runtime)| {
            let runtime = runtime.as_ref()?;
            let name = match runtime.r#type {
                RuntimeType::Direct => return None,
                RuntimeType::Docker => "docker",
                RuntimeType::Podman => "podman",
            };
            let image = runtime
                .image
                .as_deref()
                .map(|image| format!(" (image {image})"))
                .unwrap_or_default();
            Some(format!(
                "{kind} '{id}' runs in a {name} container{image}; 2.0 does not run containers, and running those steps on this machine instead is not the same"
            ))
        })
        .collect()
}

fn jssg_steps_mut(workflow: &mut Workflow) -> Vec<&mut UseJSAstGrep> {
    let mut found = Vec::new();
    let steps = workflow
        .nodes
        .iter_mut()
        .flat_map(|node| node.steps.iter_mut())
        .chain(
            workflow
                .templates
                .iter_mut()
                .flat_map(|template| template.steps.iter_mut()),
        );
    for step in steps {
        match &mut step.action {
            StepAction::JSAstGrep(js) => found.push(js),
            StepAction::Shard(shard) => {
                if let Some(js) = shard.js_ast_grep.as_mut() {
                    found.push(js);
                }
            }
            _ => {}
        }
    }
    found
}

/// The module and the package files it imports, relatively.
fn closure(entry: &Path) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    let mut queue = vec![entry.to_path_buf()];
    while let Some(file) = queue.pop() {
        if seen.len() > 500 || !seen.insert(file.clone()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        queue.extend(
            transform_module::relative_imports(&text)
                .iter()
                .filter_map(|specifier| transform_module::resolve(&dir, specifier))
                .map(|path| normalize(&path)),
        );
        order.push(file);
    }
    order
}

fn imports_metrics(text: &str) -> bool {
    text.contains("\"codemod:metrics\"") || text.contains("'codemod:metrics'")
}

/// The state functions a module imports from `codemod:workflow`.
fn state_calls(text: &str) -> Vec<&'static str> {
    if !(text.contains("\"codemod:workflow\"") || text.contains("'codemod:workflow'")) {
        return Vec::new();
    }
    STATE_API
        .iter()
        .copied()
        .filter(|name| {
            text.match_indices(name).any(|(at, _)| {
                let before = text[..at].chars().next_back();
                let after = text[at + name.len()..].chars().next();
                !before.is_some_and(is_identifier_char) && !after.is_some_and(is_identifier_char)
            })
        })
        .collect()
}

fn is_identifier_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// Every module the file imports: `import`, `export ... from`, `import()`
/// and `require()` with a literal, and `import('x')` written in a string
/// that is evaluated (`eval("import('node:fs/promises')")`). Comments do
/// not count. A file that does not parse is read as text.
fn import_specifiers(file: &Path, text: &str) -> Vec<String> {
    use oxc::allocator::Allocator;
    use oxc::ast::ast::{Argument, Expression};
    use oxc::ast::AstKind;
    use oxc::parser::Parser;
    use oxc::semantic::SemanticBuilder;
    use oxc::span::SourceType;

    let allocator = Allocator::default();
    let source_type = SourceType::from_path(file).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, text, source_type).parse();
    if parsed.panicked || !parsed.errors.is_empty() {
        return import_specifiers_in_text(text);
    }
    let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
    let mut found = Vec::new();
    for node in semantic.nodes().iter() {
        match node.kind() {
            AstKind::ImportDeclaration(import) => found.push(import.source.value.to_string()),
            AstKind::ExportNamedDeclaration(export) => {
                if let Some(source) = &export.source {
                    found.push(source.value.to_string());
                }
            }
            AstKind::ExportAllDeclaration(export) => found.push(export.source.value.to_string()),
            AstKind::ImportExpression(import) => {
                if let Expression::StringLiteral(source) = &import.source {
                    found.push(source.value.to_string());
                }
            }
            AstKind::CallExpression(call) => {
                let is_require = matches!(
                    call.callee.without_parentheses(),
                    Expression::Identifier(callee) if callee.name == "require"
                );
                if let (true, Some(Argument::StringLiteral(source))) =
                    (is_require, call.arguments.first())
                {
                    found.push(source.value.to_string());
                }
            }
            AstKind::StringLiteral(literal) if literal.value.contains("import(") => {
                found.extend(import_specifiers_in_text(&literal.value));
            }
            _ => {}
        }
    }
    found
}

/// [`import_specifiers`] for text that does not parse.
fn import_specifiers_in_text(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for keyword in ["from", "import", "require("] {
        for (at, _) in text.match_indices(keyword) {
            let rest = text[at + keyword.len()..].trim_start_matches([' ', '(', '\t']);
            let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            if let Some(end) = rest[1..].find(quote) {
                found.push(rest[1..1 + end].to_string());
            }
        }
    }
    found
}

fn capability_of(specifier: &str) -> Option<&'static str> {
    match specifier.strip_prefix("node:").unwrap_or(specifier) {
        "fs" | "fs/promises" => Some("fs"),
        "child_process" => Some("child_process"),
        _ => None,
    }
}

fn wrapper_source(relative: &str, get_selector: bool) -> String {
    let original = format!("../{relative}");
    let selector = if get_selector {
        format!("export {{ getSelector }} from \"{original}\";\n")
    } else {
        String::new()
    };
    format!(
        r#"// Added by codemod: runs {relative} as codemod 1.x did. The counts it records
// with codemod:metrics are each file's data in the step's result.
import transform from "{original}";
import {{ takeMetrics }} from "./metrics.ts";
{selector}
export default async function transformWithMetrics(root, options) {{
  takeMetrics();
  const out = await transform(root, options);
  const metrics = takeMetrics();
  if (metrics === null) return out;
  if (out !== null && typeof out === "object" && "edit" in out) {{
    return {{ edit: out.edit, data: {{ data: out.data, metrics }} }};
  }}
  return {{ edit: out ?? null, data: {{ metrics }} }};
}}
"#
    )
}

/// 1.x's `codemod:metrics`: `useMetricAtom(name).increment(cardinality?,
/// amount?)` and `getEntries()`. Counts are kept until the wrapper takes
/// them after each file, so `getEntries()` sees the current file's.
const METRICS_MODULE: &str = r#"// Added by codemod: codemod 1.x's `codemod:metrics`, which 2.0 removed. The
// counts a transform records are returned as the file's data.
const counts = new Map();

export class MetricAtom {
  constructor(name) {
    this.name = name;
  }

  increment(cardinality, amount) {
    const pairs = {};
    if (cardinality !== null && typeof cardinality === "object") {
      for (const key of Object.keys(cardinality).sort()) {
        const value = cardinality[key];
        if (value !== undefined && value !== null) pairs[key] = String(value);
      }
    }
    const by = typeof amount === "number" ? Math.trunc(amount) : 1;
    let metric = counts.get(this.name);
    if (metric === undefined) {
      metric = new Map();
      counts.set(this.name, metric);
    }
    const key = JSON.stringify(pairs);
    const entry = metric.get(key);
    if (entry === undefined) metric.set(key, { cardinality: pairs, count: by });
    else entry.count += by;
  }

  getEntries() {
    const metric = counts.get(this.name);
    if (metric === undefined) return [];
    return [...metric.values()].map((entry) => ({ cardinality: { ...entry.cardinality }, count: entry.count }));
  }
}

export function useMetricAtom(name) {
  return new MetricAtom(name);
}

/** The counts recorded since the last call, by metric name, or null. */
export function takeMetrics() {
  if (counts.size === 0) return null;
  const out = {};
  for (const [name, metric] of counts) out[name] = [...metric.values()];
  counts.clear();
  return out;
}

export default { useMetricAtom, MetricAtom };
"#;

/// `path` without `.` and `..` components.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// `file` relative to `base`, with `/`, when it is inside it.
fn relative_to(base: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(base).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(parts.join("/"))
}

fn shown(base: &Path, file: &Path) -> String {
    relative_to(base, file).unwrap_or_else(|| file.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compat::{upgrade_file, Translated};

    fn package(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    fn jssg_workflow(js_file: &str) -> String {
        format!(
            "version: \"1\"\nnodes:\n  - id: a\n    name: A\n    steps:\n      - name: Rewrite\n        js-ast-grep:\n          js_file: {js_file}\n          language: typescript\n"
        )
    }

    /// Upgrade the package's workflow.yaml, write what it writes, and check
    /// the result builds.
    fn upgraded(dir: &Path) -> Translated {
        let translated = upgrade_file(&dir.join("workflow.yaml")).unwrap();
        for (path, text) in &translated.files {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let workflow = dir.join("workflow.ts");
        std::fs::write(&workflow, &translated.source).unwrap();
        if let Err(error) = crate::engine::load_workflow(&workflow) {
            panic!("{error}\n\n{}", translated.source);
        }
        translated
    }

    const METRICS_TRANSFORM: &str = "import { useMetricAtom } from \"codemod:metrics\";\nconst seen = useMetricAtom(\"seen\");\nexport default function transform(root) {\n  seen.increment({ kind: \"x\" });\n  return null;\n}\n";

    #[test]
    fn metrics_imports_point_at_the_added_module_and_the_transform_is_wrapped() {
        let dir = package(&[
            ("workflow.yaml", &jssg_workflow("scripts/codemod.ts")),
            ("scripts/codemod.ts", METRICS_TRANSFORM),
        ]);
        let translated = upgraded(dir.path());
        let transform = std::fs::read_to_string(dir.path().join("scripts/codemod.ts")).unwrap();
        assert!(
            transform.contains("from \"../__codemod_v1/metrics.ts\""),
            "{transform}"
        );
        assert!(dir.path().join("__codemod_v1/metrics.ts").is_file());
        let wrapper =
            std::fs::read_to_string(dir.path().join("__codemod_v1/scripts__codemod.ts")).unwrap();
        assert!(
            wrapper.contains("import transform from \"../scripts/codemod.ts\""),
            "{wrapper}"
        );
        assert!(
            translated
                .source
                .contains("./__codemod_v1/scripts__codemod.ts"),
            "{}",
            translated.source
        );
        assert!(
            !translated.source.contains("jssg-compat"),
            "{}",
            translated.source
        );
        assert!(translated
            .notes
            .iter()
            .any(|n| n.contains("codemod:metrics")));
    }

    #[test]
    fn workflow_state_in_a_transform_is_an_upgrade_error() {
        let dir = package(&[
            ("workflow.yaml", &jssg_workflow("scripts/codemod.ts")),
            (
                "scripts/codemod.ts",
                "import { setState } from \"codemod:workflow\";\nexport default function transform(root) { setState(\"x\", 1); return null; }\n",
            ),
        ]);
        let message = upgrade_file(&dir.path().join("workflow.yaml")).unwrap_err();
        assert!(message.contains("cannot be upgraded"), "{message}");
        assert!(
            message.contains("scripts/codemod.ts calls setState"),
            "{message}"
        );
    }

    #[test]
    fn container_steps_are_an_upgrade_error() {
        let dir = package(&[(
            "workflow.yaml",
            "version: \"1\"\nnodes:\n  - id: build\n    name: Build\n    runtime:\n      type: docker\n      image: alpine\n    steps:\n      - name: Where\n        run: uname -s\n",
        )]);
        let message = upgrade_file(&dir.path().join("workflow.yaml")).unwrap_err();
        assert!(
            message.contains("node 'build' runs in a docker container (image alpine)"),
            "{message}"
        );
    }

    #[test]
    fn transforms_that_import_fs_ask_for_it() {
        let dir = package(&[
            ("workflow.yaml", &jssg_workflow("scripts/codemod.ts")),
            (
                "scripts/helper.ts",
                "import fs from \"fs\";\nexport const read = (p) => fs.readFileSync(p, \"utf8\");\n",
            ),
            (
                "scripts/codemod.ts",
                "import { read } from \"./helper.ts\";\nexport default function transform(root) { return null; }\n",
            ),
        ]);
        let translated = upgraded(dir.path());
        assert!(
            translated.source.contains("capabilities: [\"fs\"]"),
            "{}",
            translated.source
        );
        assert!(translated.notes.iter().any(|n| n.contains("imports fs")));
    }

    #[test]
    fn imports_in_comments_do_not_ask_for_capabilities() {
        let path = Path::new("t.ts");
        let specifiers = import_specifiers(
            path,
            "/** We also catch `const fs = require('fs');` */\nimport x from \"./x.ts\";\nconst p = (0, eval)(\"import('node:fs/promises')\");\nconst c = require(\"child_process\");\n",
        );
        assert_eq!(
            specifiers,
            vec!["./x.ts", "node:fs/promises", "child_process"]
        );
    }

    #[test]
    fn state_calls_match_whole_identifiers() {
        assert_eq!(
            state_calls("import { getState } from 'codemod:workflow';\ngetState('a');"),
            vec!["getState"]
        );
        assert!(state_calls("const getStateful = 1; // codemod:workflow").is_empty());
        assert!(state_calls("setState(1)").is_empty());
    }
}

//! Analyses as values in workflows: code that imports `callGraph`, `types`,
//! `entryPoints` or `join` from `codemod/ast-grep` gets workspace analysis
//! without naming it; `symbols` and `dataflow` follow the step's scope.

mod common;

use common::{output, Harness};
use serde_json::{json, Value};

fn harness() -> Harness {
    let h = Harness::new();
    h.write("src/lib.ts", "export function helper() {}\n");
    h.write(
        "src/a.ts",
        "import { helper } from \"./lib\";\nexport function run() {\n  helper();\n}\n",
    );
    h
}

#[test]
fn importing_an_analysis_turns_on_workspace_analysis() {
    let h = harness();
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, callGraph } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "src/lib.ts" }, (file) => {
    const { callers } = callGraph("typescript");
    const helper = parse(file).root().find("function helper() {}");
    return callers.get(helper).map((edge) => edge.site.text());
  }));
  return found.outputs.flat();
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["helper()"]));
}

#[test]
fn symbols_follow_the_step_scope_file_unless_workspace_is_asked() {
    let h = harness();
    let code = r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, symbols } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "src/lib.ts", semanticAnalysis: SCOPE }, (file) => {
    const { references } = symbols("typescript");
    const name = parse(file).root().find("function helper() {}").field("name");
    return (references.get(name) ?? []).map((edge) => edge.node.getRoot().filename().split("/").pop()).sort();
  }));
  return found.outputs.flat();
}
"#;
    let file = h.start(&code.replace("SCOPE", r#""file""#), Value::Null);
    assert_eq!(output(&file).unwrap(), json!([]));
    let workspace = h.start(&code.replace("SCOPE", r#""workspace""#), Value::Null);
    assert_eq!(output(&workspace).unwrap(), json!(["a.ts", "a.ts"]));
}

#[test]
fn a_plain_step_can_join_and_crawl_the_workspace() {
    let h = harness();
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { callGraph, crawl, join } from "codemod/ast-grep";
export default async function main() {
  return await step(async () => {
    const exported = join({
      language: "typescript",
      from: { find: "export function $F() { $$$ }", on: "$F" },
      to: { find: "function $F() { $$$ }", on: "$F" },
    });
    const { callees } = callGraph("typescript");
    const starts = [...exported.values()].flat().map((edge) => edge.node);
    const reached = await crawl(callees, starts);
    return [...reached.keys()].map((node) => node.text().match(/function (\w+)/)[1]).sort();
  });
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["helper", "run"]));
}

#[test]
fn crawling_from_the_entry_points_leaves_out_dead_code() {
    let h = Harness::new();
    h.write(
        "packages/ui/package.json",
        r#"{ "name": "@acme/ui", "exports": { ".": "./src/index.ts" } }"#,
    );
    h.write(
        "packages/ui/src/index.ts",
        "export { Button } from \"./button\";\n",
    );
    h.write(
        "packages/ui/src/button.ts",
        "export function Button() { return helper(); }\nfunction helper() { return 1; }\nfunction unused() { return helper(); }\n",
    );
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { callGraph, crawl, entryPoints } from "codemod/ast-grep";
export default async function main() {
  return await step(async () => {
    const live = await crawl(callGraph("typescript").callees, entryPoints("typescript"));
    return [...live.keys()].map((node) => node.text().match(/function (\w+)/)[1]);
  });
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["Button", "helper"]));
}

#[test]
fn entry_points_say_why_each_function_is_called_from_outside() {
    let h = Harness::new();
    h.write(
        "packages/ui/package.json",
        r#"{ "name": "@acme/ui", "exports": { ".": "./src/index.ts" } }"#,
    );
    h.write(
        "packages/ui/src/index.ts",
        "export function Button() { return 1; }\nfunction helper() { return 2; }\n",
    );
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { entryPoints } from "codemod/ast-grep";
export default async function main() {
  return await step(() => {
    const outside = entryPoints("typescript");
    return [...outside].map(([fn, why]) => [fn.field("name").text(), why]);
  });
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!([["Button", [{ "kind": "export", "label": "@acme/ui#Button" }]]])
    );
}

#[test]
fn join_links_python_routes_to_their_views() {
    let h = Harness::new();
    h.write(
        "shop/urls.py",
        "from django.urls import path\nfrom . import views\n\nurlpatterns = [\n    path(\"orders/\", views.order_list),\n]\n",
    );
    h.write(
        "shop/views.py",
        "def order_list(request):\n    return load_orders()\n\n\ndef load_orders():\n    return []\n\n\ndef unrouted(request):\n    return None\n",
    );
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { join } from "codemod/ast-grep";
export default async function main() {
  return await step(() => {
    const routes = join({
      language: "python",
      from: { find: "path($ROUTE, views.$VIEW)", on: "$VIEW" },
      to: { find: { rule: { kind: "function_definition", has: { field: "name", pattern: "$VIEW" } } }, on: "$VIEW" },
      meta: { via: "django-url" },
    });
    return [...routes].map(([route, [edge]]) => `${route.text()} -> ${edge.name} (${edge.via})`);
  });
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!(["path(\"orders/\", views.order_list) -> order_list (django-url)"])
    );
}

#[test]
fn a_node_a_step_returns_is_its_key_and_a_later_step_asks_about_it() {
    let h = harness();
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, callGraph } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "src/a.ts" }, (file) =>
    parse(file).root().find("function run() { $$$ }"),
  ));
  const run = found.outputs[0];
  return await step(() => ({
    isKey: typeof run === "string" && run.endsWith(":function_declaration"),
    callees: callGraph("typescript").callees.get(run).map((edge) => edge.site.text()),
  }));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "isKey": true, "callees": ["helper()"] })
    );
}

#[test]
fn a_lookup_a_step_returns_is_a_lookup_again_in_a_later_step() {
    let h = harness();
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { join, lookup } from "codemod/ast-grep";
export default async function main() {
  const exports = await step(() =>
    join({
      language: "typescript",
      from: { find: "export function $F() { $$$ }", on: "$F" },
      to: { find: "function $F() { $$$ }", on: "$F" },
      meta: { via: "export" },
    }),
  );
  return await step(() =>
    [...lookup(exports)].map(([from, [edge]]) =>
      [from.kind(), edge.node.kind(), edge.site.kind(), edge.name, edge.via].join(" "),
    ).sort(),
  );
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!([
            "export_statement function_declaration export_statement helper export",
            "export_statement function_declaration export_statement run export",
        ])
    );
}

#[test]
fn a_crawl_result_a_step_returns_keeps_where_each_node_came_from() {
    let h = harness();
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, callGraph, crawl, lookup } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "src/a.ts" }, (file) =>
    parse(file).root().find("function run() { $$$ }"),
  ));
  const reached = await step(() => crawl(callGraph("typescript").callees, found.outputs[0]));
  return await step(() => {
    const name = (node) => node.text().match(/function (\w+)/)[1];
    return [...lookup(reached)].map(([node, edge]) =>
      edge ? `${name(edge.from)} -> ${name(node)} at ${edge.site.text()}` : `start ${name(node)}`,
    );
  });
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!(["start run", "run -> helper at helper()"])
    );
}

#[test]
fn a_key_from_before_its_file_changed_fails_and_says_so() {
    let h = harness();
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, lookup } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "src/a.ts" }, (file) =>
    parse(file).root().find("function run() { $$$ }"),
  ));
  await step(forEachFile({ include: "src/a.ts" }, (file) => {
    file.write(`// moved\n${file.text}`);
  }));
  const run = found.outputs[0];
  return await step(() => lookup([[run, run]]).size);
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("no node for")
            && error.contains("the file changed after the step that made the key"),
        "{error}"
    );
}

#[test]
fn in_a_dry_run_keys_name_nodes_as_earlier_steps_left_the_files() {
    let h = harness();
    let report = h.dry_run(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, lookup } from "codemod/ast-grep";
export default async function main() {
  await step(forEachFile({ include: "src/a.ts" }, (file) => {
    file.write(`// moved\n${file.text}`);
  }));
  const found = await step(forEachFile({ include: "src/a.ts" }, (file) =>
    parse(file).root().find("function run() { $$$ }"),
  ));
  const run = found.outputs[0];
  return await step(() => [...lookup([[run, run]]).keys()].map((node) => node.text().split("\n")[0]));
}
"#,
    );
    assert_eq!(output(&report).unwrap(), json!(["function run() {"]));
    assert!(h.read("src/a.ts").starts_with("import"));
}

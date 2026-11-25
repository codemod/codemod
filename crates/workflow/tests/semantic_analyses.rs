//! Analyses as values in workflows: code that imports `callGraph`, `join` or
//! `crawl` from `codemod/ast-grep` gets workspace analysis without naming it.

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

//! Python through the analyses: what `types("python")` turns on, module
//! lookups in `symbols("python")`, and the entry points Python code has.

mod common;

use common::{output, Harness};
use serde_json::{json, Value};

fn shop() -> Harness {
    let h = Harness::new();
    h.write(
        "shop/repo.py",
        "class Repo:\n    def save(self, order):\n        pass\n",
    );
    h.write(
        "shop/checkout.py",
        "from shop.repo import Repo\n\ndef save_all(repos: list[Repo], order):\n    for repo in repos:\n        repo.save(order)\n        first = repos[0]\n",
    );
    h
}

#[test]
fn type_of_is_what_ty_infers() {
    let h = shop();
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, types } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "shop/checkout.py" }, (file) => {
    const { typeOf } = types("python");
    return typeOf.get(parse(file).root().find("first"));
  }));
  return found.outputs;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["Repo"]));
}

const SAVE_CALLEES: &str = r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, callGraph, types } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "shop/checkout.py" }, (file) => {
    TYPES
    const { callees } = callGraph("python");
    const def = parse(file).root().find({ rule: { kind: "function_definition" } });
    return (callees.get(def) ?? []).map((edge) => `${edge.node.find({ rule: { kind: "identifier" } }).text()} (${edge.via})`);
  }));
  return found.outputs.flat();
}
"#;

#[test]
fn with_types_the_call_graph_resolves_calls_by_the_receivers_type() {
    let report = shop().start(
        &SAVE_CALLEES.replace("TYPES", "types(\"python\");"),
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["save (type)"]));
}

#[test]
fn without_types_a_call_only_the_type_explains_is_left_out() {
    let report = shop().start(&SAVE_CALLEES.replace("TYPES", ""), Value::Null);
    assert_eq!(output(&report).unwrap(), json!([]));
}

fn layers() -> Harness {
    let h = Harness::new();
    h.write("app/__init__.py", "");
    h.write("app/db.py", "def connect():\n    pass\n");
    h.write("app/orders.py", "from app.db import connect\n");
    h.write("app/views.py", "from . import orders\n");
    h.write("app/settings.py", "DEBUG = True\n");
    h
}

#[test]
fn crawling_importers_finds_every_file_that_depends_on_a_module() {
    let report = layers().start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, symbols, crawl } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "app/db.py", semanticAnalysis: "workspace" }, async (file) => {
    const reached = await crawl(symbols("python").importers, [parse(file).root()]);
    return [...reached.keys()].map((root) => root.getRoot().filename().split("/").pop()).sort();
  }));
  return found.outputs.flat();
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!(["db.py", "orders.py", "views.py"])
    );
}

#[test]
fn imports_land_on_the_modules_a_file_names_with_the_statement_as_site() {
    let report = layers().start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, symbols } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "app/views.py", semanticAnalysis: "workspace" }, (file) => {
    const edges = symbols("python").imports.get(parse(file).root()) ?? [];
    return edges.map((e) => `${e.site.text()} -> ${e.node.getRoot().filename().split("/").pop()}`);
  }));
  return found.outputs.flat();
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!([
            "from . import orders -> __init__.py",
            "from . import orders -> orders.py"
        ])
    );
}

#[test]
fn crawling_from_python_entry_points_reaches_what_routes_call_and_not_dead_code() {
    let h = Harness::new();
    h.write(
        "api/main.py",
        "from fastapi import FastAPI\n\napp = FastAPI()\n\n\n@app.get(\"/orders\")\ndef list_orders():\n    return load_orders()\n\n\ndef load_orders():\n    return []\n\n\ndef unused():\n    return load_orders()\n",
    );
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { callGraph, crawl, entryPoints } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "api/main.py" }, async () => {
    const live = await crawl(callGraph("python").callees, entryPoints("python"));
    return [...live.keys()].map((node) => node.text().match(/def (\w+)/)[1]);
  }));
  return found.outputs.flat();
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!(["list_orders", "load_orders"])
    );
}

#[test]
fn crawling_sources_stops_at_a_sanitizer_the_value_was_reassigned_through() {
    let h = Harness::new();
    h.write(
        "shop/search.py",
        "def search(request, cursor):\n    q = request.GET[\"q\"]\n    q = clean(q)\n    cursor.execute(q)\n",
    );
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, dataflow, crawl } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "shop/search.py" }, async (file) => {
    const root = parse(file).root();
    const sink = root.find("cursor.execute($Q)").getMatch("Q");
    const cleaned = root.findAll("clean($$$)");
    const reached = await crawl(dataflow("python").sources, [sink], { stopAt: cleaned });
    return [...reached.keys()].map((node) => node.text());
  }));
  return found.outputs.flat();
}
"#,
        Value::Null,
    );
    let reached = output(&report).unwrap();
    assert!(reached.as_array().unwrap().contains(&json!("clean(q)")));
    assert!(!reached
        .as_array()
        .unwrap()
        .contains(&json!("request.GET[\"q\"]")));
}

/// A signal sent in one module, received in another, both importing it.
fn signals() -> Harness {
    let h = Harness::new();
    h.write("pkg/__init__.py", "");
    h.write(
        "pkg/signals.py",
        "from x import Signal\nplaced = Signal()\n",
    );
    h.write(
        "pkg/checkout.py",
        "from pkg.signals import placed\n\ndef place():\n    placed.send()\n",
    );
    h.write(
        "pkg/receivers.py",
        "from x import receiver\nfrom pkg.signals import placed\n\n@receiver(placed)\ndef on_placed():\n    pass\n",
    );
    h
}

#[test]
fn definition_of_an_imported_name_lands_in_the_module_that_declares_it() {
    let report = signals().start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { parse, symbols } from "codemod/ast-grep";
export default async function main() {
  const found = await step(forEachFile({ include: "pkg/checkout.py", semanticAnalysis: "workspace" }, (file) => {
    const [def] = symbols("python").definition.get(parse(file).root().find("placed.send()").field("function").field("object")) ?? [];
    return `${def.node.getRoot().filename().split("/").pop()}:${def.node.range().start.line + 1} ${def.kind}`;
  }));
  return found.outputs;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["signals.py:2 external"]));
}

#[test]
fn join_by_default_joins_a_send_to_the_receiver_of_the_same_imported_signal() {
    let report = signals().start(
        r#"
import { step } from "codemod/workflow";
import { join } from "codemod/ast-grep";
export default async function main() {
  return await step("join", () => {
    const sent = join({
      language: "python",
      from: { find: "$S.send($$$)", anchor: "function", on: "$S" },
      to: { find: { rule: { kind: "function_definition", inside: { kind: "decorated_definition", has: { kind: "decorator", has: { pattern: "receiver($S)" } } } } }, on: "$S" },
    });
    return [...sent].flatMap(([from, edges]) => edges.map((e) => `${from.field("name").text()} -> ${e.node.field("name").text()}`));
  });
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["place -> on_placed"]));
}

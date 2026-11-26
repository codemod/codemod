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

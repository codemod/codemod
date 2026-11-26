//! Rust through the analyses: `callGraph`, `dataflow`, `symbols`, `types`,
//! `entryPoints` and `join` answer for `.rs` files with rust-analyzer.
//!
//! One workflow run on one small library crate, `shop` (`src/lib.rs` prices
//! items through a trait, `src/tax.rs` adds VAT), holds every case below:
//! each `test(name, ...)` checks one rule and reports as its own Rust test,
//! so rust-analyzer loads the crate once, not once per rule.

mod common;

use common::{output, Harness};
use serde_json::Value;
use std::sync::OnceLock;

const LIB: &str = r#"pub mod tax;
use tax::vat;

pub trait Item {
    fn price(&self) -> u32;
}

pub struct Book;
pub struct Pen;

impl Item for Book {
    fn price(&self) -> u32 {
        vat(10)
    }
}

impl Item for Pen {
    fn price(&self) -> u32 {
        2
    }
}

pub fn total(items: &[&dyn Item]) -> u32 {
    let prices: Vec<u32> = items.iter().map(|item| item.price()).collect();
    let mut sum = 0;
    for item in items {
        sum += item.price();
    }
    sum + prices.len() as u32
}

fn unused() -> u32 {
    vat(1)
}
"#;

const TAX: &str = r#"pub fn vat(amount: u32) -> u32 {
    amount + amount / 5
}
"#;

fn shop() -> Harness {
    let h = Harness::new();
    h.write(
        "Cargo.toml",
        "[package]\nname = \"shop\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    h.write("src/lib.rs", LIB);
    h.write("src/tax.rs", TAX);
    h
}

/// The cases. `test(name, { file, scope }, fn)` runs `fn(root, file)` in a
/// step over `file` with that scope (`workspace` unless it says `file`);
/// `expect(actual, expected)` fails the case with both values.
const CASES: &str = r#"
test("callers are the call sites in every file", { file: "src/tax.rs" }, (root) => {
  const { callers } = callGraph("rust");
  expect(callers.get(fnNamed(root, "vat")).map((edge) => where(edge.site)), [
    "src/lib.rs vat(10)",
    "src/lib.rs vat(1)",
  ]);
});

test("a call through a trait object reaches every impl", { file: "src/lib.rs" }, (root) => {
  const { callees } = callGraph("rust");
  const impls = callees
    .get(fnNamed(root, "total"))
    .filter((edge) => edge.via === "type")
    .map((edge) => where(edge.node).split(" {")[0]);
  expect(impls, ["src/lib.rs fn price(&self) -> u32", "src/lib.rs fn price(&self) -> u32"]);
});

test("crawling sources from a variable reaches every value assigned to it", { file: "src/lib.rs" }, async (root) => {
  const { sources } = dataflow("rust");
  const sum = root.find({ rule: { pattern: "sum + prices.len() as u32" } }).field("left");
  const reached = [...(await crawl(sources, [sum])).keys()].map((node) => node.text());
  // `let mut sum = 0;` and `sum += item.price();`
  expect(["0", "item.price()"].filter((value) => reached.includes(value)), ["0", "item.price()"]);
});

test("a definition is the declaration in the file that holds it", { file: "src/lib.rs" }, (root) => {
  const { definition } = symbols("rust");
  const call = root.find({ rule: { pattern: "vat(10)" } }).field("function");
  expect(definition.get(call).map((edge) => where(edge.node)), ["src/tax.rs vat"]);
});

test("in file scope a name from another file has no definition", { file: "src/lib.rs", scope: "file" }, (root) => {
  const { definition } = symbols("rust");
  const call = root.find({ rule: { pattern: "vat(10)" } }).field("function");
  expect(definition.get(call), undefined);
});

test("references are every use including the use item", { file: "src/tax.rs" }, (root) => {
  const { references } = symbols("rust");
  const name = fnNamed(root, "vat").field("name");
  expect(references.get(name).map((edge) => where(edge.node)), ["src/lib.rs vat", "src/lib.rs vat", "src/lib.rs vat"]);
});

test("a module path resolves to the file that holds the module", { file: "src/lib.rs" }, (root, file) => {
  const { resolveModule } = symbols("rust");
  expect(resolveModule(file, "crate::tax").split("/").slice(-2).join("/"), "src/tax.rs");
});

test("the path to a module file starts at the crate", { file: "src/lib.rs" }, (root, file) => {
  const { specifierFor } = symbols("rust");
  expect(specifierFor(file, "src/tax.rs"), "crate::tax");
});

test("types are what rust-analyzer infers", { file: "src/lib.rs" }, (root) => {
  const { typeOf } = types("rust");
  const prices = root
    .find({ rule: { kind: "let_declaration", has: { field: "pattern", regex: "^prices$" } } })
    .field("pattern");
  expect(typeOf.get(prices), "Vec<u32>");
});

test("entry points are the public api of the library with its path", { file: "src/lib.rs" }, () => {
  const found = [...entryPoints("rust").entries()]
    .map(([fn, reasons]) => fn.field("name").text() + ": " + reasons.map((r) => r.kind + " " + r.label).join(", "))
    .sort();
  expect(found, ["price: export shop::Item::price", "total: export shop::total", "vat: export shop::tax::vat"]);
});

test("join links a call to its function through the definition", { file: "src/lib.rs" }, () => {
  const linked = join({
    language: "rust",
    from: { find: "$F($$$ARGS)", on: "$F" },
    to: { find: "pub fn $F($$$PARAMS) -> $R { $$$BODY }", on: "$F" },
  });
  expect(
    [...linked.entries()].map(([call, edges]) => call.text() + " -> " + edges.map((e) => where(e.node).split(" {")[0]).join(", ")),
    ["vat(10) -> src/tax.rs pub fn vat(amount: u32) -> u32", "vat(1) -> src/tax.rs pub fn vat(amount: u32) -> u32"],
  );
});

test("crawling callees from the entry points leaves out dead code", { file: "src/lib.rs" }, async (root) => {
  const { callees } = callGraph("rust");
  const reached = await crawl(callees, entryPoints("rust").keys());
  const live = new Set([...reached.keys()].map((node) => node.key()));
  const dead = root
    .findAll({ rule: { kind: "function_item" } })
    .filter((fn) => !live.has(fn.key()))
    .map((fn) => fn.field("name").text());
  expect(dead, ["unused"]);
});
"#;

/// The workflow that runs [`CASES`]: one `forEachFile` step per scope, each
/// running the cases for the file it is on. Each case answers `"ok"` or why
/// it failed.
fn workflow() -> String {
    format!(
        r#"
import {{ step, forEachFile }} from "codemod/workflow";
import {{ parse, callGraph, crawl, dataflow, symbols, types, entryPoints, join }} from "codemod/ast-grep";
function fnNamed(root, name) {{
  return root.find({{ rule: {{ kind: "function_item", has: {{ field: "name", regex: "^" + name + "$" }} }} }});
}}
function where(node) {{
  return node.getRoot().filename().split("/").slice(-2).join("/") + " " + node.text();
}}
function expect(actual, expected) {{
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {{
    throw new Error("expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual));
  }}
}}
async function run(scope, file) {{
  const root = parse(file).root();
  const cases = [];
  const test = (name, at, fn) => {{
    if ((at.scope ?? "workspace") === scope && root.getRoot().filename().endsWith(at.file)) cases.push([name, fn]);
  }};
  {CASES}
  const results = {{}};
  for (const [name, fn] of cases) {{
    try {{
      await fn(root, file);
      results[name] = "ok";
    }} catch (error) {{
      results[name] = String(error?.message ?? error);
    }}
  }}
  return results;
}}
export default async function main() {{
  const workspace = await step(forEachFile({{ include: "src/*.rs", semanticAnalysis: "workspace" }}, (file) => run("workspace", file)));
  const fileScope = await step(forEachFile({{ include: "src/*.rs", semanticAnalysis: "file" }}, (file) => run("file", file)));
  return Object.assign({{}}, ...workspace.outputs, ...fileScope.outputs);
}}
"#
    )
}

/// What each case answered, from the one run.
fn results() -> &'static Value {
    static RESULTS: OnceLock<Value> = OnceLock::new();
    RESULTS.get_or_init(|| {
        let h = shop();
        let report = h.start(&workflow(), Value::Null);
        output(&report).unwrap()
    })
}

fn passes(name: &str) {
    match results().get(name).and_then(Value::as_str) {
        Some("ok") => {}
        Some(why) => panic!("{name}: {why}"),
        None => panic!("{name}: no such case ran"),
    }
}

macro_rules! cases {
    ($($test:ident => $name:literal,)*) => {
        $(
            #[test]
            fn $test() {
                passes($name);
            }
        )*

        #[test]
        fn every_case_is_reported() {
            let reported = [$($name),*];
            let ran = results().as_object().unwrap();
            for name in ran.keys() {
                assert!(reported.contains(&name.as_str()), "{name} runs but no Rust test reports it");
            }
        }
    };
}

cases! {
    callers_are_the_call_sites_in_every_file => "callers are the call sites in every file",
    a_call_through_a_trait_object_reaches_every_impl => "a call through a trait object reaches every impl",
    crawling_sources_from_a_variable_reaches_every_value_assigned_to_it => "crawling sources from a variable reaches every value assigned to it",
    a_definition_is_the_declaration_in_the_file_that_holds_it => "a definition is the declaration in the file that holds it",
    in_file_scope_a_name_from_another_file_has_no_definition => "in file scope a name from another file has no definition",
    references_are_every_use_including_the_use_item => "references are every use including the use item",
    a_module_path_resolves_to_the_file_that_holds_the_module => "a module path resolves to the file that holds the module",
    the_path_to_a_module_file_starts_at_the_crate => "the path to a module file starts at the crate",
    types_are_what_rust_analyzer_infers => "types are what rust-analyzer infers",
    entry_points_are_the_public_api_of_the_library_with_its_path => "entry points are the public api of the library with its path",
    join_links_a_call_to_its_function_through_the_definition => "join links a call to its function through the definition",
    crawling_callees_from_the_entry_points_leaves_out_dead_code => "crawling callees from the entry points leaves out dead code",
}

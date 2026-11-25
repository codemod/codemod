//! `callers()` returns call sites only. Whether a function nothing in the
//! workspace calls is still called from outside is its own question:
//! `entryPoints()` (its package exports it, or a rule on the selector
//! declares it).

mod common;

use common::{output, Harness};
use serde_json::{json, Value};

/// `@acme/ui` exports `Button` through a barrel; `helper` is only called by
/// `Button`, and `unused` by nothing.
fn package() -> Harness {
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
        "export function Button() { return helper(); }\nfunction helper() { return 1; }\nexport function unused() {}\n",
    );
    h
}

/// Runs `query(fn)` on the function named `name` in `file`.
fn ask(h: &Harness, file: &str, name: &str, semantic: &str, query: &str) -> Value {
    let function = if file.ends_with(".py") {
        "function_definition"
    } else {
        "function_declaration"
    };
    let workflow = format!(
        r#"
import {{ step, forEachFile }} from "codemod/workflow";
import {{ parse }} from "codemod/ast-grep";
export default async function main() {{
  const found = await step(forEachFile({{ include: "{file}", semanticAnalysis: {semantic} }}, (file) => {{
    const fn = parse(file).root().find({{ rule: {{ kind: "identifier", regex: "^{name}$", inside: {{ kind: "{function}" }} }} }});
    return ({query})(fn);
  }}));
  return found.outputs[0];
}}
"#
    );
    output(&h.start(&workflow, Value::Null)).unwrap()
}

#[test]
fn callers_never_returns_an_entry_point() {
    let h = package();
    let callers = ask(
        &h,
        "packages/ui/src/button.ts",
        "Button",
        r#""workspace""#,
        "(fn) => fn.callers().length",
    );
    assert_eq!(callers, json!(0));
}

#[test]
fn an_export_from_a_public_specifier_is_an_entry_point() {
    let h = package();
    let entry_points = |name| {
        ask(
            &h,
            "packages/ui/src/button.ts",
            name,
            r#""workspace""#,
            "(fn) => fn.entryPoints()",
        )
    };
    assert_eq!(
        entry_points("Button"),
        json!([{ "kind": "export", "label": "@acme/ui#Button" }])
    );
    assert_eq!(entry_points("helper"), json!([]));
    assert_eq!(
        entry_points("unused"),
        json!([]),
        "exported from a file the package doesn't"
    );
}

#[test]
fn a_declared_route_export_is_an_entry_point() {
    let h = Harness::new();
    h.write(
        "app/api/users/[id]/route.ts",
        "export function GET() {}\nexport function helper() {}\n",
    );
    let semantic = r#"{ mode: "workspace", entryPoints: [{ files: "app/**/route.ts", exports: ["GET", "POST"], kind: "http" }] }"#;
    let entry_points = |name| {
        ask(
            &h,
            "app/api/users/[id]/route.ts",
            name,
            semantic,
            "(fn) => fn.entryPoints()",
        )
    };
    assert_eq!(
        entry_points("GET"),
        json!([{ "kind": "http", "label": "GET app/api/users/[id]/route.ts" }])
    );
    assert_eq!(entry_points("helper"), json!([]));
}

#[test]
fn a_declared_decorator_is_an_entry_point() {
    let h = Harness::new();
    h.write(
        "api/views.py",
        "@app.route(\"/users\")\ndef users():\n    return []\n\ndef plain():\n    return []\n",
    );
    let semantic =
        r#"{ mode: "workspace", entryPoints: [{ decorators: ["*.route"], kind: "http" }] }"#;
    let entry_points = |name| {
        ask(
            &h,
            "api/views.py",
            name,
            semantic,
            "(fn) => fn.entryPoints()",
        )
    };
    assert_eq!(
        entry_points("users"),
        json!([{ "kind": "http", "label": "@app.route api/views.py:users" }])
    );
    assert_eq!(entry_points("plain"), json!([]));
}

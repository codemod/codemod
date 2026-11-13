//! Test runs: stopping after a step (`until`) and skipping steps with a given
//! result (`given`).

mod common;

use codemod_workflow::{Ending, RunStatus, TestPlan};
use common::Harness;
use serde_json::{json, Map, Value};

const THREE_STEPS: &str = r#"
import { step, forEachFile } from "codemod:workflow";
export default async function main() {
  const found = await step("Find files", forEachFile("**/*.ts", (file) => ({ path: file.path })));
  await step("Rewrite", forEachFile("**/*.ts", (file) => {
    file.write(file.text.replace("old", "new"));
  }));
  await step("Rename", forEachFile("**/*.ts", (file) => file.rename(file.path.replace(".ts", ".mts"))));
  return { found: found.outputs.map((o) => o.path) };
}
"#;

fn plan(until: Option<&str>, given: Value) -> TestPlan {
    TestPlan {
        until: until.map(str::to_string),
        given: match given {
            Value::Object(map) => map,
            _ => Map::new(),
        },
    }
}

#[test]
fn until_stops_after_the_named_step() {
    let h = Harness::new();
    h.write("a.ts", "old\n");

    let report = h.test(THREE_STEPS, plan(Some("Rewrite"), Value::Null));

    assert_eq!(report.ending, Ending::Stopped("Rewrite".into()));
    assert_eq!(report.run.status, RunStatus::Succeeded);
    let labels: Vec<_> = report.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["Find files", "Rewrite"]);
    assert_eq!(h.read("a.ts"), "new\n");
    assert!(!h.exists("a.mts"));
}

#[test]
fn steps_started_alongside_the_until_step_still_finish() {
    let h = Harness::new();
    h.write("a.ts", "old\n");

    let report = h.test(
        r#"
import { step, forEachFile } from "codemod:workflow";
export default async function main() {
  const before = step("Before", forEachFile("**/*.ts", () => {}));
  await Promise.all([before, step("Stop here", forEachFile("**/*.ts", () => {}))]);
  await step("After", forEachFile("**/*.ts", (file) => file.write("changed")));
}
"#,
        plan(Some("Stop here"), Value::Null),
    );

    assert_eq!(report.ending, Ending::Stopped("Stop here".into()));
    let labels: Vec<_> = report.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["Before", "Stop here"]);
    assert_eq!(h.read("a.ts"), "old\n");
}

#[test]
fn a_given_step_does_not_run_and_later_steps_see_its_result() {
    let h = Harness::new();
    h.write("a.ts", "old\n");

    let report = h.test(
        THREE_STEPS,
        plan(
            Some("Rewrite"),
            json!({ "Find files": { "outputs": [{ "path": "given.ts" }] } }),
        ),
    );

    assert_eq!(report.ending, Ending::Stopped("Rewrite".into()));
    assert!(report.steps[0].given);
    assert_eq!(
        report.steps[0].result,
        Some(json!({ "outputs": [{ "path": "given.ts" }] }))
    );
    assert!(!report.steps[1].given);
}

#[test]
fn a_run_without_until_returns_the_workflow_output() {
    let h = Harness::new();
    h.write("a.ts", "old\n");

    let report = h.test(
        THREE_STEPS,
        plan(
            None,
            json!({ "Find files": { "outputs": [{ "path": "x.ts" }] } }),
        ),
    );

    assert_eq!(
        report.ending,
        Ending::Succeeded(json!({ "found": ["x.ts"] }))
    );
    assert_eq!(h.read("a.mts"), "new\n");
}

#[test]
fn an_until_step_that_fails_fails_the_run() {
    let h = Harness::new();

    let report = h.test(
        r#"
import { step } from "codemod:workflow";
export default async function main() {
  await step("Boom", () => { throw new Error("no"); });
}
"#,
        plan(Some("Boom"), Value::Null),
    );

    assert!(matches!(report.ending, Ending::Failed(ref e) if e.contains("no")));
}

#[test]
fn test_runs_are_not_persisted() {
    let h = Harness::new();
    h.write("a.ts", "old\n");

    h.test(THREE_STEPS, plan(None, Value::Null));

    assert!(h.store.list_runs(10).unwrap().is_empty());
}

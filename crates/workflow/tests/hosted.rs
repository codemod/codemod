//! Runs whose body another host (Node.js) runs: the engine side of it.

mod common;

use codemod_workflow::{Ending, OpenRequest, RunStatus, StepRecord, StepStatus, TestPlan};
use common::Harness;
use serde_json::{json, Value};

fn open(h: &Harness, test: Option<TestPlan>) -> codemod_workflow::HostedRun {
    h.bundle_file("workflow.ts", "export default async function main() {}\n");
    h.engine
        .open(OpenRequest {
            workflow: h.bundle.path().join("workflow.ts"),
            target: h.target.path().to_path_buf(),
            params: json!({ "to": "new" }),
            name: None,
            dry_run: false,
            test,
        })
        .unwrap()
}

fn succeeded(position: u64, label: &str, result: Value) -> StepRecord {
    serde_json::from_value(json!({
        "position": position,
        "label": label,
        "key": "{}",
        "status": "succeeded",
        "result": result,
    }))
    .unwrap()
}

#[test]
fn a_hosted_run_selects_commits_and_records_steps() {
    let h = Harness::new();
    h.write("src/a.ts", "old\n");
    h.write("src/b.js", "old\n");
    let run = open(&h, None);
    assert_eq!(run.params(), &json!({ "to": "new" }));

    let (language, files) = run
        .select_files(json!({ "include": ["**/*.ts"], "exclude": [] }))
        .unwrap();
    assert_eq!(language, None);
    assert_eq!(files, vec![("src/a.ts".to_string(), "old\n".to_string())]);

    let committed = run
        .commit_files(
            0,
            "Rewrite".into(),
            json!([
                { "path": "src/a.ts", "original": "old\n", "effects": [
                    { "op": "write", "text": "new\n" },
                    { "op": "rename", "to": "src/a.mts" },
                ] },
                { "path": "src/b.js", "original": "old\n", "effects": [{ "op": "rename", "to": "../escape.js" }] },
            ]),
        )
        .unwrap();
    assert_eq!(
        committed["changes"]["renamed"],
        json!([{ "from": "src/a.ts", "to": "src/a.mts" }])
    );
    assert_eq!(committed["failures"][0]["path"], json!("src/b.js"));
    assert_eq!(h.read("src/a.mts"), "new\n");
    assert!(!h.exists("src/a.ts"));

    run.record(&succeeded(0, "Rewrite", committed.clone()), 5)
        .unwrap();
    let report = h
        .engine
        .finish(&run, Ending::Succeeded(json!({ "done": true })))
        .unwrap();
    assert_eq!(report.run.status, RunStatus::Succeeded);
    assert_eq!(report.steps.len(), 1);
    assert_eq!(report.steps[0].status, StepStatus::Succeeded);
    assert!(h.engine.finish(&run, Ending::Canceled).is_err());
}

#[test]
fn a_hosted_run_runs_host_tasks() {
    let h = Harness::new();
    let run = open(&h, None);
    let outcome = run
        .run_task(
            &json!({
                "position": 0,
                "label": "Echo",
                "task": { "kind": "shell", "command": "echo hosted" },
                "state": {},
            })
            .to_string(),
        )
        .unwrap();
    let outcome: Value = serde_json::from_str(&outcome).unwrap();
    assert_eq!(
        outcome["result"]["output"].as_str().unwrap().trim(),
        "hosted"
    );

    let error = run
        .run_task(
            &json!({ "position": 1, "label": "Nope", "task": { "kind": "nope" } }).to_string(),
        )
        .unwrap_err();
    assert!(error.contains("unknown task kind 'nope'"), "{error}");
}

#[test]
fn a_hosted_test_run_that_reached_its_until_step_ends_as_stopped() {
    let h = Harness::new();
    let run = open(
        &h,
        Some(TestPlan {
            until: Some("First".into()),
            given: Default::default(),
        }),
    );
    assert_eq!(run.test_plan().unwrap().until.as_deref(), Some("First"));
    run.stop();
    let report = h
        .engine
        .finish(&run, Ending::Succeeded(Value::Null))
        .unwrap();
    assert_eq!(report.ending, Ending::Stopped("First".into()));
    assert_eq!(report.run.status, RunStatus::Succeeded);
}

#[test]
fn resuming_a_hosted_run_returns_its_history() {
    let h = Harness::new();
    let run = open(&h, None);
    run.record(&succeeded(0, "One", json!(1)), 0).unwrap();
    let report = h
        .engine
        .finish(&run, Ending::Failed("later step failed".into()))
        .unwrap();
    assert_eq!(report.run.status, RunStatus::Failed);

    let resumed = h.engine.open_resumed(&report.run.id).unwrap();
    assert_eq!(resumed.history().len(), 1);
    assert_eq!(resumed.history()[0].label, "One");
}

//! What a run reports when it ends: its output in `RunFinished` (also for
//! runs that `fail()`), and the check for uncommitted changes before the
//! first write.

mod common;

use std::process::Command;
use std::sync::{Arc, Mutex};

use codemod_workflow::events::Event;
use codemod_workflow::interaction::{Approval, Decision, Interaction, NonInteractive};
use codemod_workflow::{Ending, MemoryStore, RunStatus, StartRequest};
use common::Harness;
use serde_json::{json, Value};

fn finished(harness: &Harness) -> Event {
    harness
        .events
        .take()
        .into_iter()
        .rfind(|event| matches!(event, Event::RunFinished { .. }))
        .expect("no RunFinished")
}

const CHECK: &str = r#"
import { fail, step } from "codemod/workflow";
export default async function main() {
  const found = await step("Find", () => ["a.ts"]);
  const report = { checks: [{ id: "no-var", status: "fail", files: found }] };
  fail("1 check failed", { output: report });
}
"#;

#[test]
fn run_finished_has_the_output_and_the_step_results() {
    let harness = Harness::new();
    let report = harness.start(
        r#"
import { step } from "codemod/workflow";
export default async function main() {
  const n = await step("Count", () => 2);
  return { n };
}
"#,
        Value::Null,
    );
    assert_eq!(report.ending, Ending::Succeeded(json!({ "n": 2 })));
    let Event::RunFinished {
        output,
        output_omitted,
        steps,
        ..
    } = finished(&harness)
    else {
        unreachable!()
    };
    assert_eq!(output, json!({ "n": 2 }));
    assert_eq!(output_omitted, None);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].label, "Count");
    assert_eq!(steps[0].result, json!(2));
}

#[test]
fn a_dry_run_reports_its_output_too() {
    let harness = Harness::new();
    harness.dry_run("export default async function main() { return [1, 2]; }\n");
    let Event::RunFinished { output, .. } = finished(&harness) else {
        unreachable!()
    };
    assert_eq!(output, json!([1, 2]));
}

#[test]
fn fail_ends_the_run_failed_with_its_output() {
    let harness = Harness::new();
    let report = harness.start(CHECK, Value::Null);
    let expected = json!({ "checks": [{ "id": "no-var", "status": "fail", "files": ["a.ts"] }] });
    assert_eq!(
        report.ending,
        Ending::FailedWithOutput {
            error: "1 check failed".into(),
            output: expected.clone(),
            exit_code: 2,
        }
    );
    assert_eq!(report.run.status, RunStatus::Failed);
    assert_eq!(report.run.error.as_deref(), Some("1 check failed"));
    assert_eq!(report.run.output, Some(expected.clone()));
    assert_eq!(report.run.exit_code, Some(2));
    assert_eq!(report.to_json()["output"], expected);
    assert_eq!(report.to_json()["exitCode"], 2);

    let Event::RunFinished {
        status,
        error,
        output,
        ..
    } = finished(&harness)
    else {
        unreachable!()
    };
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(error.as_deref(), Some("1 check failed"));
    assert_eq!(output, expected);
}

#[test]
fn fail_takes_an_exit_code_from_1_to_125() {
    let harness = Harness::new();
    let report = harness.start(
        r#"
import { fail } from "codemod/workflow";
export default async function main() { fail("no", { exitCode: 7 }); }
"#,
        Value::Null,
    );
    assert_eq!(
        report.ending,
        Ending::FailedWithOutput {
            error: "no".into(),
            output: Value::Null,
            exit_code: 7,
        }
    );

    for bad in ["0", "126", "1.5", "\"2\""] {
        let report = harness.start(
            &format!(
                "import {{ fail }} from \"codemod/workflow\";\nexport default async function main() {{ fail(\"no\", {{ exitCode: {bad} }}); }}\n"
            ),
            Value::Null,
        );
        let Ending::Failed(error) = &report.ending else {
            panic!("{bad}: {:?}", report.ending)
        };
        assert!(error.contains("exitCode"), "{bad}: {error}");
    }
}

#[test]
fn fail_inside_a_step_is_a_build_error() {
    let error = codemod_workflow::build(
        "workflow.ts",
        r#"
import { fail, step } from "codemod/workflow";
export default async function main() {
  await step("Inner", () => fail("inner", { output: 1 }));
}
"#,
    )
    .expect_err("expected a build error")
    .to_string();
    assert!(
        error.contains("uses 'fail' inside a function step"),
        "{error}"
    );
}

#[test]
fn a_failed_run_resumed_to_success_forgets_the_exit_code() {
    let harness = Harness::with(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: false }),
    );
    let report = harness.start(CHECK, Value::Null);
    assert_eq!(report.run.exit_code, Some(2));
    let resumed = harness.resume(
        &report,
        Some(&CHECK.replace(
            "fail(\"1 check failed\", { output: report });",
            "return report;",
        )),
    );
    assert_eq!(resumed.run.status, RunStatus::Succeeded);
    assert_eq!(resumed.run.exit_code, None);
}

// ------------------------------------------------- uncommitted changes ---

/// Says yes or no to the uncommitted-changes question and notes it was asked.
struct Answer {
    yes: bool,
    asked: Mutex<Vec<Approval>>,
}

impl Interaction for Answer {
    fn is_interactive(&self) -> bool {
        true
    }

    fn approve(&self, approval: &Approval) -> Decision {
        self.asked.lock().unwrap().push(approval.clone());
        match approval {
            Approval::UncommittedChanges { .. } if !self.yes => Decision::Denied,
            _ => Decision::Approved,
        }
    }
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "user.email=t@t", "-c", "user.name=t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}");
}

fn dirty(harness: &Harness) {
    harness.write("a.ts", "var a = 1;\n");
    git(harness.target.path(), &["init", "-q"]);
    git(harness.target.path(), &["add", "."]);
    git(harness.target.path(), &["commit", "-qm", "init"]);
    harness.write("b.ts", "uncommitted\n");
}

fn checked_start(harness: &Harness, source: &str) -> codemod_workflow::RunReport {
    harness
        .engine
        .start(StartRequest {
            run_id: None,
            workflow: harness.workflow(source),
            target: harness.target.path().to_path_buf(),
            params: Value::Null,
            name: None,
            dry_run: false,
            test: None,
            check_uncommitted: true,
        })
        .unwrap()
}

const READS: &str = r#"
import { forEachFile, step } from "codemod/workflow";
export default async function main() {
  return await step("Read", forEachFile("*.ts", (file) => file.text.length));
}
"#;

const WRITES: &str = r#"
import { forEachFile, step } from "codemod/workflow";
export default async function main() {
  await step("Read", forEachFile("*.ts", (file) => file.text.length));
  await step("Write", forEachFile("a.ts", (file) => file.write("let a = 1;\n")));
}
"#;

#[test]
fn a_run_that_only_reads_never_asks_about_uncommitted_changes() {
    let answer = Arc::new(Answer {
        yes: false,
        asked: Mutex::default(),
    });
    let harness = Harness::with(Arc::new(MemoryStore::default()), answer.clone());
    dirty(&harness);
    let report = checked_start(&harness, READS);
    assert!(
        matches!(report.ending, Ending::Succeeded(_)),
        "{:?}",
        report.ending
    );
    assert!(answer.asked.lock().unwrap().is_empty());
}

#[test]
fn the_first_write_asks_about_uncommitted_changes_once() {
    let answer = Arc::new(Answer {
        yes: true,
        asked: Mutex::default(),
    });
    let harness = Harness::with(Arc::new(MemoryStore::default()), answer.clone());
    dirty(&harness);
    let twice = WRITES.replace(
        "\n}\n",
        "\n  await step(\"Again\", forEachFile(\"b.ts\", (file) => file.write(\"x\\n\")));\n}\n",
    );
    let report = checked_start(&harness, &twice);
    assert!(
        matches!(report.ending, Ending::Succeeded(_)),
        "{:?}",
        report.ending
    );
    assert_eq!(harness.read("a.ts"), "let a = 1;\n");
    let asked = answer.asked.lock().unwrap();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(matches!(
        &asked[0],
        Approval::UncommittedChanges {
            not_a_repository: false,
            ..
        }
    ));
}

#[test]
fn a_no_stops_the_run_before_it_writes() {
    let answer = Arc::new(Answer {
        yes: false,
        asked: Mutex::default(),
    });
    let harness = Harness::with(Arc::new(MemoryStore::default()), answer);
    dirty(&harness);
    let report = checked_start(&harness, WRITES);
    let Ending::Failed(error) = &report.ending else {
        panic!("{:?}", report.ending)
    };
    assert!(error.contains("uncommitted changes"), "{error}");
    assert!(error.contains("--allow-dirty"), "{error}");
    assert_eq!(harness.read("a.ts"), "var a = 1;\n");
}

#[test]
fn unattended_runs_never_write_over_uncommitted_changes() {
    let harness = Harness::with(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: true }),
    );
    dirty(&harness);
    let report = checked_start(&harness, WRITES);
    assert!(
        matches!(report.ending, Ending::Failed(_)),
        "{:?}",
        report.ending
    );
    assert_eq!(harness.read("a.ts"), "var a = 1;\n");
}

#[test]
fn a_clean_tree_is_written_without_asking() {
    let answer = Arc::new(Answer {
        yes: false,
        asked: Mutex::default(),
    });
    let harness = Harness::with(Arc::new(MemoryStore::default()), answer.clone());
    dirty(&harness);
    std::fs::remove_file(harness.target.path().join("b.ts")).unwrap();
    let report = checked_start(&harness, WRITES);
    assert!(
        matches!(report.ending, Ending::Succeeded(_)),
        "{:?}",
        report.ending
    );
    assert!(answer.asked.lock().unwrap().is_empty());
}

#[test]
fn a_git_step_without_managed_git_is_not_a_write() {
    let answer = Arc::new(Answer {
        yes: false,
        asked: Mutex::default(),
    });
    let harness = Harness::with(Arc::new(MemoryStore::default()), answer.clone());
    dirty(&harness);
    let report = checked_start(
        &harness,
        r#"
import { step } from "codemod/workflow";
import { branch, commit } from "codemod/git";
export default async function main() {
  await step(branch("codemod/x"));
  await step(commit("nothing"));
  return "read only";
}
"#,
    );
    assert_eq!(report.ending, Ending::Succeeded(json!("read only")));
    assert!(answer.asked.lock().unwrap().is_empty());
}

#[test]
fn a_resumed_run_that_never_wrote_checks_again() {
    let answer = Arc::new(Answer {
        yes: false,
        asked: Mutex::default(),
    });
    let harness = Harness::with(Arc::new(MemoryStore::default()), answer.clone());
    dirty(&harness);
    let report = checked_start(&harness, WRITES);
    assert!(matches!(report.ending, Ending::Failed(_)));
    assert!(report.run.settings.check_uncommitted);

    // Still dirty: the resume stops at the same place, and writes nothing.
    let resumed = harness.resume(&report, None);
    let Ending::Failed(error) = &resumed.ending else {
        panic!("{:?}", resumed.ending)
    };
    assert!(error.contains("uncommitted changes"), "{error}");
    assert_eq!(harness.read("a.ts"), "var a = 1;\n");
    assert_eq!(answer.asked.lock().unwrap().len(), 2);

    // Committed: the resume writes, and the run no longer checks.
    git(harness.target.path(), &["add", "."]);
    git(harness.target.path(), &["commit", "-qm", "b"]);
    let resumed = harness.resume(&report, None);
    assert!(
        matches!(resumed.ending, Ending::Succeeded(_)),
        "{:?}",
        resumed.ending
    );
    assert_eq!(harness.read("a.ts"), "let a = 1;\n");
    assert!(!resumed.run.settings.check_uncommitted);
}

#[test]
fn a_resumed_run_that_already_wrote_is_not_checked_again() {
    let answer = Arc::new(Answer {
        yes: true,
        asked: Mutex::default(),
    });
    let harness = Harness::with(Arc::new(MemoryStore::default()), answer.clone());
    dirty(&harness);
    let failing = WRITES.replace(
        "\n}\n",
        "\n  await step(\"Check\", () => { throw new Error(\"not yet\"); });\n}\n",
    );
    let report = checked_start(&harness, &failing);
    assert!(matches!(report.ending, Ending::Failed(_)));
    assert_eq!(harness.read("a.ts"), "let a = 1;\n");
    assert!(!report.run.settings.check_uncommitted);

    let resumed = harness.resume(
        &report,
        Some(&failing.replace("throw new Error(\"not yet\");", "return 1;")),
    );
    assert!(
        matches!(resumed.ending, Ending::Succeeded(_)),
        "{:?}",
        resumed.ending
    );
    assert_eq!(answer.asked.lock().unwrap().len(), 1);
}

#[test]
fn resuming_with_allow_dirty_skips_the_check() {
    let answer = Arc::new(Answer {
        yes: false,
        asked: Mutex::default(),
    });
    let harness = Harness::with(Arc::new(MemoryStore::default()), answer.clone());
    dirty(&harness);
    let report = checked_start(&harness, WRITES);
    assert!(matches!(report.ending, Ending::Failed(_)));
    let resumed = harness
        .engine
        .resume_with(
            &report.run.id,
            codemod_workflow::ResumeOptions {
                allow_dirty: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        matches!(resumed.ending, Ending::Succeeded(_)),
        "{:?}",
        resumed.ending
    );
    assert_eq!(answer.asked.lock().unwrap().len(), 1);
    assert!(!resumed.run.settings.check_uncommitted);
}

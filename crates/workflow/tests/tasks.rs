mod common;

use std::sync::Arc;

use codemod_workflow::events::{Event, PartialStep};
use codemod_workflow::interaction::{Decision, NonInteractive};
use codemod_workflow::{
    cancel_run, Canceled, Ending, FileCount, MemoryStore, RunStatus, Services, StepStatus,
};
use common::{output, Harness};
use serde_json::{json, Value};

fn approving() -> Harness {
    Harness::with(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: true }),
    )
}

#[test]
fn shell_runs_in_the_target_by_default() {
    let h = Harness::new();
    h.write("cwd-marker.txt", "target");
    let report = h.start(
        r#"
import { step, shell } from "codemod/workflow";
export default async function main() {
  return (await step(shell("echo ok < cwd-marker.txt"))).exitCode;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(0));
}

#[cfg(unix)]
#[test]
fn rename_cannot_write_through_an_external_symlink() {
    let h = Harness::new();
    let outside = tempfile::tempdir().unwrap();
    h.write("a.txt", "original");
    std::os::unix::fs::symlink(outside.path(), h.target.path().join("link")).unwrap();
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step(forEachFile("a.txt", (file) => file.rename("link/new/file.txt")));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("must stay inside the target"), "{error}");
    assert_eq!(h.read("a.txt"), "original");
    assert!(!outside.path().join("new").exists());
}

#[cfg(unix)]
#[test]
fn shell_honors_cwd_for_shebang_scripts() {
    let h = Harness::new();
    h.write("sub dir/cwd-marker.txt", "subdir");
    let report = h.start(
        r##"
import { step, shell } from "codemod/workflow";
export default async function main() {
  return (await step(shell("#!/bin/sh\ncat cwd-marker.txt", { cwd: "sub dir" }))).stdout;
}
"##,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!("subdir"));
}

#[test]
fn a_shell_step_returns_what_the_command_printed() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step, shell } from "codemod/workflow";
export default async function main() {
  const { stdout, stderr, exitCode } = await step(shell(
    'echo "$GREETING" && echo careful >&2 && echo \'{"count":2}\'',
    { env: { GREETING: "hi" } },
  ));
  const lines = stdout.trim().split("\n");
  return { first: lines[0], parsed: JSON.parse(lines.at(-1)), stderr, exitCode };
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "first": "hi", "parsed": { "count": 2 }, "stderr": "careful\n", "exitCode": 0 })
    );
}

#[test]
fn a_failing_shell_command_fails_the_step_with_what_it_printed() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step, shell } from "codemod/workflow";
export default async function main() { await step("Fail", shell("echo nope >&2 && exit 3")); }
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("step 'Fail' failed"), "{error}");
    assert!(error.contains("exited with code 3:\nnope"), "{error}");
}

#[test]
fn a_shell_step_that_allows_failure_returns_the_exit_code() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step, shell } from "codemod/workflow";
export default async function main() {
  return (await step(shell("exit 3", { allowFailure: true }))).exitCode;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(3));
}

#[test]
fn shell_steps_are_skipped_in_a_dry_run() {
    let h = Harness::new();
    let report = h.dry_run(
        r#"
import { step, shell } from "codemod/workflow";
export default async function main() { return step(shell("touch should-not-exist")); }
"#,
    );
    assert_eq!(output(&report).unwrap(), json!({ "skipped": true }));
    assert!(!h.exists("should-not-exist"));
}

const NEEDS_APPROVAL: &str = r#"
import { step, approval, forEachFile } from "codemod/workflow";
export default async function main() {
  await step(forEachFile("*.txt", (file) => { file.write(file.text + "1"); }));
  await step(approval("Ship it?"));
  await step(forEachFile("*.txt", (file) => { file.write(file.text + "2"); }));
}
"#;

#[test]
fn an_approval_nobody_can_give_pauses_the_run_until_resumed() {
    let store = Arc::new(MemoryStore::default());
    let h = Harness::with(
        store.clone(),
        Arc::new(NonInteractive { approve_all: false }),
    );
    h.write("a.txt", "a");

    let first = h.start(NEEDS_APPROVAL, Value::Null);
    assert_eq!(first.ending, Ending::AwaitingApproval("Ship it?".into()));
    assert_eq!(first.run.status, RunStatus::AwaitingApproval);
    assert_eq!(first.run.awaiting.as_deref(), Some("Ship it?"));
    assert_eq!(h.read("a.txt"), "a1");
    // The step that waits is on record, so `status` can name it.
    assert_eq!(first.steps.len(), 2);
    assert_eq!(first.steps[1].status, StepStatus::AwaitingApproval);
    // And the question, so `resume --approve <id>` can name it.
    assert_eq!(first.run.question.as_deref(), Some("approval:1"));

    // Resume with approval, sharing the same store.
    let approver = Harness::with(store, Arc::new(NonInteractive { approve_all: true }));
    let second = approver.engine.resume(&first.run.id).unwrap();
    assert!(output(&second).is_ok(), "{:?}", second.ending);
    assert_eq!(h.read("a.txt"), "a12");
}

#[test]
fn canceling_a_run_that_waits_for_approval_marks_it_canceled() {
    let h = Harness::new();
    h.write("a.txt", "a");
    let first = h.start(NEEDS_APPROVAL, Value::Null);
    assert_eq!(
        cancel_run(h.store.as_ref(), &first.run.id).unwrap(),
        Canceled::Marked
    );
    let run = h.store.get_run(&first.run.id).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Canceled);
    assert!(run.awaiting.is_none());
    assert_eq!(
        cancel_run(h.store.as_ref(), &first.run.id).unwrap(),
        Canceled::AlreadyFinished(RunStatus::Canceled)
    );
}

/// Approves the questions it is given by id, rejects the rest, and keeps
/// what it was asked.
struct ById {
    yes: Vec<&'static str>,
    asked: std::sync::Mutex<Vec<(String, Option<String>)>>,
}

impl ById {
    fn new(yes: &[&'static str]) -> Arc<Self> {
        Arc::new(Self {
            yes: yes.to_vec(),
            asked: Default::default(),
        })
    }
}

impl codemod_workflow::interaction::Interaction for ById {
    fn is_interactive(&self) -> bool {
        true
    }

    fn approve(&self, _approval: &codemod_workflow::interaction::Approval) -> Decision {
        unreachable!("the engine asks by id")
    }

    fn ask(
        &self,
        ask: &codemod_workflow::interaction::Ask,
        _approval: &codemod_workflow::interaction::Approval,
    ) -> Decision {
        self.asked
            .lock()
            .unwrap()
            .push((ask.id.clone(), ask.step.clone()));
        if self.yes.contains(&ask.id.as_str()) {
            Decision::Approved
        } else {
            Decision::Denied
        }
    }
}

const TWO_APPROVALS: &str = r#"
import { step, approval, forEachFile } from "codemod/workflow";
export default async function main() {
  await step(forEachFile("*.txt", (file) => { file.write(file.text + "1"); }));
  await step(approval("Go on?"));
  await step(forEachFile("*.txt", (file) => { file.write(file.text + "2"); }));
  await step(approval("Finish?"));
  await step(forEachFile("*.txt", (file) => { file.write(file.text + "3"); }));
}
"#;

#[test]
fn a_rejected_approval_ends_the_run_as_rejected_and_keeps_earlier_changes() {
    let store = Arc::new(MemoryStore::default());
    let asker = ById::new(&["approval:1"]);
    let h = Harness::with(store.clone(), asker.clone());
    h.write("a.txt", "a");

    let report = h.start(TWO_APPROVALS, Value::Null);
    assert_eq!(report.ending, Ending::Rejected("Finish?".into()));
    assert_eq!(report.run.status, RunStatus::Rejected);
    assert_eq!(report.run.error.as_deref(), Some("rejected: Finish?"));
    assert_eq!(h.read("a.txt"), "a12");
    assert_eq!(report.steps[3].status, StepStatus::Rejected);
    assert_eq!(report.steps.len(), 4, "nothing ran after the rejection");
    assert_eq!(
        *asker.asked.lock().unwrap(),
        [
            ("approval:1".to_string(), Some("1".to_string())),
            ("approval:3".to_string(), Some("3".to_string()))
        ]
    );
    let finished = h
        .events
        .take()
        .into_iter()
        .rfind(|event| matches!(event, Event::RunFinished { .. }))
        .unwrap();
    assert!(
        matches!(
            &finished,
            Event::RunFinished {
                status: RunStatus::Rejected,
                ..
            }
        ),
        "{finished:?}"
    );

    // Over, unless forced.
    let error = h.engine.resume(&report.run.id).unwrap_err().to_string();
    assert!(
        error.contains("rejected") && error.contains("--force"),
        "{error}"
    );
    let approver = Harness::with(store, ById::new(&["approval:3"]));
    let resumed = approver
        .engine
        .resume_with(
            &report.run.id,
            codemod_workflow::ResumeOptions {
                force: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(output(&resumed).is_ok(), "{:?}", resumed.ending);
    assert_eq!(h.read("a.txt"), "a123");
}

#[test]
fn a_rejection_stops_steps_running_alongside() {
    let h = Harness::with(Arc::new(MemoryStore::default()), ById::new(&[]));
    let report = h.start(
        r#"
import { step, approval } from "codemod/workflow";
export default async function main() {
  await Promise.all([
    step("Spin", () => { while (true) {} }),
    step(approval("Stop it?")),
  ]);
}
"#,
        Value::Null,
    );
    assert_eq!(report.ending, Ending::Rejected("Stop it?".into()));
    assert_eq!(report.run.status, RunStatus::Rejected);
}

#[test]
fn a_canceled_run_is_resumed_only_when_forced() {
    let store = Arc::new(MemoryStore::default());
    let h = Harness::with(
        store.clone(),
        Arc::new(NonInteractive { approve_all: false }),
    );
    h.write("a.txt", "a");
    let first = h.start(NEEDS_APPROVAL, Value::Null);
    cancel_run(h.store.as_ref(), &first.run.id).unwrap();
    let approver = Harness::with(store, Arc::new(NonInteractive { approve_all: true }));
    assert!(matches!(
        approver.engine.resume(&first.run.id),
        Err(codemod_workflow::EngineError::Ended {
            status: RunStatus::Canceled,
            ..
        })
    ));
    assert_eq!(h.read("a.txt"), "a1");
    let second = approver
        .engine
        .resume_with(
            &first.run.id,
            codemod_workflow::ResumeOptions {
                force: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(output(&second).is_ok(), "{:?}", second.ending);
    assert_eq!(h.read("a.txt"), "a12");
}

#[test]
fn ast_grep_steps_apply_rule_fixes() {
    let h = Harness::new();
    h.write("a.js", "var x = 1;\n");
    h.bundle_file(
        "rules.yaml",
        "id: no-var\nlanguage: javascript\nrule:\n  pattern: var $A = $B\nfix: let $A = $B;\n",
    );
    let report = h.start(
        r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
import rules from "./rules.yaml";
export default async function main() { return step(forEachFile(astGrep(rules))); }
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(h.read("a.js"), "let x = 1;\n");
    assert_eq!(result["changes"]["written"], json!(["a.js"]));
}

#[test]
fn shard_steps_group_files_and_feed_later_steps() {
    let h = Harness::new();
    for dir in ["one", "two"] {
        for n in 0..3 {
            h.write(&format!("src/{dir}/f{n}.ts"), "export {}\n");
        }
    }
    let report = h.start(
        r#"
import { step, shard } from "codemod/workflow";
export default async function main() {
  const shards = await step(shard({
    method: { type: "directory", maxFilesPerShard: 10 },
    target: "src",
    files: "**/*.ts",
  }));
  return shards.map((s: any) => [s.name, s._meta_files.length]);
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(result.as_array().unwrap().len(), 2, "{result}");
    assert!(result.to_string().contains("one"), "{result}");
}

/// `[name, files]` of each shard a workflow returned.
fn shard_files(result: &Value) -> Vec<(String, Vec<String>)> {
    result
        .as_array()
        .unwrap()
        .iter()
        .map(|shard| {
            let files = shard["_meta_files"].as_array().unwrap();
            (
                shard["name"].as_str().unwrap().to_string(),
                files
                    .iter()
                    .map(|f| f.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn codeowner_shards_group_by_the_last_matching_rule_and_cap_files() {
    let h = Harness::new();
    // docs/CODEOWNERS is the third place looked in; the last match wins and
    // its first owner owns the file.
    h.write(
        "docs/CODEOWNERS",
        "# owners\n/packages/ui/ @acme/ui @acme/other\npackages/api @acme/API_Team\n*.md @docs\n/packages/ui/f3.ts @someone\n",
    );
    for file in [
        "ui/f1.ts", "ui/f2.ts", "ui/f3.ts", "ui/f4.ts", "api/a.ts", "api/b.md",
    ] {
        h.write(&format!("packages/{file}"), "export {}\n");
    }
    h.write("root.ts", "export {}\n");
    h.write(".git/HEAD", "ref: refs/heads/main\n");
    let report = h.start(
        r#"
import { step, shard } from "codemod/workflow";
export default async function main() {
  return step(shard({ method: { type: "codeowner", maxFilesPerShard: 2 }, files: "**/*.{ts,md}" }));
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        shard_files(&result),
        vec![
            ("acme-api_team-0".into(), names(&["packages/api/a.ts"])),
            (
                "acme-ui-0".into(),
                names(&["packages/ui/f1.ts", "packages/ui/f2.ts"])
            ),
            ("acme-ui-1".into(), names(&["packages/ui/f4.ts"])),
            ("docs-0".into(), names(&["packages/api/b.md"])),
            ("someone-0".into(), names(&["packages/ui/f3.ts"])),
            ("unowned-0".into(), names(&["root.ts"])),
        ],
        "{result}"
    );
    assert_eq!(result[0]["team"], json!("@acme/API_Team"));
}

#[test]
fn built_in_shards_without_files_take_every_file_under_target() {
    let h = Harness::new();
    for file in ["a/one.ts", "a/two.css", "b/three.md", "top.txt"] {
        h.write(&format!("packages/{file}"), "x\n");
    }
    h.write("other.ts", "x\n");
    h.write(".git/config", "[core]\n");
    // Not UTF-8: not a file a callback could read, so in no shard.
    std::fs::write(
        h.target.path().join("packages/a/logo.png"),
        [0x89, 0x50, 0xff, 0xfe, 0x00],
    )
    .unwrap();
    let report = h.start(
        r#"
import { step, shard } from "codemod/workflow";
export default async function main() {
  return [
    await step("Packages", shard({ method: { type: "directory", maxFilesPerShard: 5 }, target: "packages" })),
    await step("Everything", shard({ method: { type: "directory", maxFilesPerShard: 5 } })),
  ];
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    let packages = shard_files(&result[0]);
    let names: Vec<&str> = packages.iter().map(|(name, _)| name.as_str()).collect();
    // Files directly under `target` are the "." group.
    assert_eq!(names, [".-0", "a-0", "b-0"], "{result}");
    assert_eq!(packages[0].1, ["packages/top.txt"], "{result}");
    assert_eq!(
        packages[1].1,
        ["packages/a/one.ts", "packages/a/two.css"],
        "{result}"
    );
    let everything = result[1].to_string();
    assert!(everything.contains("other.ts"), "{everything}");
    assert!(!everything.contains(".git"), "{everything}");
    assert!(!everything.contains("logo.png"), "{everything}");
}

#[test]
fn a_shard_target_outside_the_run_target_fails_before_reading_it() {
    let h = Harness::new();
    h.write("a/one.ts", "x\n");
    for target in ["../outside", "/etc"] {
        let report = h.start(
            &format!(
                r#"
import {{ step, shard }} from "codemod/workflow";
export default async function main() {{
  return step(shard({{ method: {{ type: "directory", maxFilesPerShard: 5 }}, target: {target:?} }}));
}}
"#
            ),
            Value::Null,
        );
        let error = output(&report).unwrap_err();
        assert!(
            error.contains("must be relative and stay within the run's target"),
            "{error}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_shard_target_that_is_a_symlink_out_of_the_run_target_fails() {
    let h = Harness::new();
    let outside = tempfile::TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret.ts"), "x\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), h.target.path().join("out")).unwrap();
    for target in ["out", ".//out/"] {
        let report = h.start(
            &format!(
                r#"
import {{ step, shard }} from "codemod/workflow";
export default async function main() {{
  return step(shard({{ method: {{ type: "directory", maxFilesPerShard: 5 }}, target: {target:?} }}));
}}
"#
            ),
            Value::Null,
        );
        let error = output(&report).unwrap_err();
        assert!(
            error.contains("must be relative and stay within the run's target"),
            "{target}: {error}"
        );
    }
}

#[test]
fn shards_given_previous_keep_their_files_and_new_files_get_new_shards() {
    let h = Harness::new();
    for n in 0..3 {
        h.write(&format!("src/ui/f{n}.ts"), "export {}\n");
    }
    let source = r#"
import { step, shard } from "codemod/workflow";
export default async function main({ params }: any) {
  return step(shard({
    method: { type: "directory", maxFilesPerShard: 2 },
    target: "src",
    files: "**/*.ts",
    previous: params.previous,
  }));
}
"#;
    let first = output(&h.start(source, json!({}))).unwrap();
    assert_eq!(
        shard_files(&first),
        vec![
            (
                "ui-0".into(),
                vec!["src/ui/f0.ts".to_string(), "src/ui/f1.ts".into()]
            ),
            ("ui-1".into(), vec!["src/ui/f2.ts".to_string()]),
        ]
    );
    std::fs::remove_file(h.target.path().join("src/ui/f0.ts")).unwrap();
    h.write("src/ui/a.ts", "export {}\n");
    let second = output(&h.start(source, json!({ "previous": first }))).unwrap();
    assert_eq!(
        shard_files(&second),
        vec![
            ("ui-0".into(), vec!["src/ui/f1.ts".to_string()]),
            ("ui-1".into(), vec!["src/ui/f2.ts".to_string()]),
            ("ui-2".into(), vec!["src/ui/a.ts".to_string()]),
        ],
        "{second}"
    );
}

#[test]
fn codemod_steps_run_a_local_package_and_detect_cycles() {
    let h = Harness::new();
    h.write("a.txt", "a");
    h.bundle_file(
        "child/workflow.ts",
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main({ params }: any) {
  await step(forEachFile("*.txt", (file) => { file.write(file.text + "!"); }));
  return params.from;
}
"#,
    );
    let report = h.start(
        r#"
import { step, codemod } from "codemod/workflow";
export default async function main() {
  return step(codemod("./child", { params: { from: "parent" } }));
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!({ "output": "parent" }));
    assert_eq!(h.read("a.txt"), "a!");

    h.bundle_file(
        "loop/workflow.ts",
        r#"
import { step, codemod } from "codemod/workflow";
export default async function main() { return step(codemod("./")); }
"#,
    );
    let report = h.start(
        r#"
import { step, codemod } from "codemod/workflow";
export default async function main() { return step(codemod("./loop")); }
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("codemod dependency cycle"), "{error}");
}

#[test]
fn a_shard_filter_that_fails_on_every_file_fails_the_step() {
    let h = Harness::new();
    h.write("src/a.ts", "const a = 1;\n");
    let report = h.start(
        r#"
import { step, shard } from "codemod/workflow";
export default async function main() {
  return step(shard({
    method: { type: "directory", maxFilesPerShard: 10 },
    target: "src",
    files: "**/*.ts",
    filter: () => { throw new Error("filter failure"); },
  }));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("shard's filter failed on every file"),
        "{error}"
    );
    assert!(error.contains("filter failure"), "{error}");
}

#[test]
fn git_steps_are_skipped_unless_managed_git_is_on() {
    let h = approving();
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { branch, commit, pullRequest } from "codemod/git";
export default async function main() {
  return [
    await step(branch("codemod/test")),
    await step(commit("chore: test")),
    await step(pullRequest({ title: "test" })),
  ];
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!([{ "skipped": true }, { "skipped": true }, { "skipped": true }])
    );
}

/// A harness with managed git on, whose target is a git repository with one
/// commit on `main`.
fn managed_git() -> Harness {
    let h = Harness::with_services(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: true }),
        Services {
            managed_git: true,
            ..Services::default()
        },
    );
    h.write("README.md", "base\n");
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.email", "test@example.com"],
        &["config", "user.name", "Test"],
        &["config", "commit.gpgsign", "false"],
        &["add", "."],
        &["commit", "-q", "-m", "base"],
    ] {
        git(&h, args);
    }
    h
}

fn git(h: &Harness, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(h.target.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

const PARALLEL_SHARDS: &str = r#"
import { step, approval, forEachFile } from "codemod/workflow";
import { branch, commit } from "codemod/git";
export default async function main() {
  await Promise.all(["a", "b"].map(async (name) => {
    await step(approval(`Migrate ${name}?`));
    await step(branch(`codemod-${name}`));
    await step(forEachFile("README.md", (file) => { file.write(file.text + name + "\n"); }));
    await step(commit(`migrate ${name}`));
  }));
}
"#;

#[test]
fn managed_git_steps_refuse_to_run_in_parallel() {
    let starts_with_branch =
        PARALLEL_SHARDS.replace("    await step(approval(`Migrate ${name}?`));\n", "");
    for source in [PARALLEL_SHARDS, starts_with_branch.as_str()] {
        let h = managed_git();
        let report = h.start(source, Value::Null);
        let error = output(&report).unwrap_err();
        assert!(
            error.contains("managed git steps can't run in parallel"),
            "{error}"
        );
        // No commit landed anywhere, on any shard's branch.
        assert_eq!(git(&h, &["rev-list", "--all", "--count"]), "1", "{source}");
    }
}

#[test]
fn parallel_git_steps_are_refused_even_with_managed_git_off() {
    let h = approving();
    let report = h.start(PARALLEL_SHARDS, Value::Null);
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("managed git steps can't run in parallel"),
        "{error}"
    );
}

#[test]
fn each_git_step_is_refused_alongside_other_steps() {
    for task in [
        "branch(\"codemod-x\")",
        "commit(\"x\")",
        "pullRequest({ title: \"x\" })",
    ] {
        let h = approving();
        let source = format!(
            r#"
import {{ step, shell }} from "codemod/workflow";
import {{ branch, commit, pullRequest }} from "codemod/git";
export default async function main() {{
  await Promise.all([step(shell("echo hi")), step({task})]);
}}
"#
        );
        let error = output(&h.start(&source, Value::Null)).unwrap_err();
        assert!(
            error.contains("managed git steps can't run in parallel"),
            "{task}: {error}"
        );
    }
}

#[test]
fn managed_git_steps_run_one_shard_after_another() {
    let h = managed_git();
    let report = h.start(
        r#"
import { step, shell, forEachFile } from "codemod/workflow";
import { branch, commit } from "codemod/git";
export default async function main() {
  // Steps without git still run together.
  await Promise.all([
    step(shell("echo one > one.txt")),
    step(shell("echo two > two.txt")),
  ]);
  for (const name of ["a", "b"]) {
    await step(branch(`codemod-${name}`));
    await step(forEachFile("README.md", (file) => { file.write(file.text + name + "\n"); }));
    await step(commit(`migrate ${name}`));
  }
}
"#,
        Value::Null,
    );
    output(&report).unwrap();
    assert_eq!(
        git(&h, &["log", "-1", "--format=%s", "codemod-a"]),
        "migrate a"
    );
    assert_eq!(git(&h, &["show", "codemod-a:README.md"]), "base\na");
    assert_eq!(
        git(&h, &["log", "-1", "--format=%s", "codemod-b"]),
        "migrate b"
    );
    assert_eq!(git(&h, &["show", "codemod-b:README.md"]), "base\na\nb");
    assert_eq!(git(&h, &["show", "codemod-a:one.txt"]), "one");
}

/// TypeScript wrappers around a callback (`satisfies`, `as`, `!`, `<T>`,
/// parentheses) are only types: the build extracts the function inside.
#[test]
fn callbacks_wrapped_in_type_assertions_are_extracted() {
    let h = Harness::new();
    h.write(
        "package-lock.json",
        r#"{ "name": "app", "lockfileVersion": 3, "packages": { "": { "name": "app", "version": "1.0.0" } } }"#,
    );
    h.write("a.ts", "a\n");
    h.write("b.ts", "b\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { dependencies } from "codemod/deps";
type Graph = { workspaces(): unknown[] };
export default async function main() {
  const roots = await step("Graph", () => (((g: Graph) => g.workspaces().length) satisfies (g: Graph) => number)(dependencies()));
  const lengths = await step("Lengths", forEachFile(
    "*.ts",
    ((file: { text: string }) => file.text.length) as (file: { text: string }) => number,
    ((total: number, length: number) => total + length)!,
    0,
  ));
  const cast = await step("Cast", forEachFile("*.ts", <(file: { path: string }) => string>((file) => file.path)));
  const inline = await step("Inline", ((() => 7) as () => number));
  return [roots, lengths.reduced, cast.outputs, inline];
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([1, 4, ["a.ts", "b.ts"], 7]));
}

/// Files that failed in a nested codemod are failed files of the run that
/// called it.
#[test]
fn a_nested_codemod_step_is_partial_when_some_of_its_files_failed() {
    let h = Harness::new();
    h.write("a.ts", "a\n");
    h.write("b.ts", "b\n");
    h.bundle_file(
        "child/workflow.ts",
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("Child", forEachFile("*.ts", (file) => {
    if (file.path === "a.ts") throw new Error("cannot handle a");
    file.write("bx\n");
  }));
}
"#,
    );
    let report = h.start(
        r#"
import { step, codemod } from "codemod/workflow";
export default async function main() {
  await step("Nested", codemod("./child"));
}
"#,
        Value::Null,
    );
    output(&report).unwrap();
    assert_eq!(h.read("b.ts"), "bx\n");
    let nested = &report.steps[0];
    assert_eq!(nested.status, StepStatus::Partial);
    assert_eq!(nested.error.as_deref(), Some("1 of 2 files failed"));
    assert_eq!(report.failed_files(), 1);
}

#[test]
fn a_nested_codemod_run_alongside_other_steps_refuses_managed_git() {
    let h = managed_git();
    h.bundle_file(
        "child/workflow.ts",
        r#"
import { step } from "codemod/workflow";
import { branch } from "codemod/git";
export default async function main() {
  await step(branch("codemod-child"));
}
"#,
    );
    let report = h.start(
        r#"
import { step, codemod, shell } from "codemod/workflow";
export default async function main() {
  await Promise.all([step(shell("echo hi")), step(codemod("./child"))]);
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("in a codemod step that ran alongside other steps"),
        "{error}"
    );
    assert_eq!(git(&h, &["branch", "--list", "codemod-child"]), "");

    // Alone, the same codemod checks out its branch.
    let report = h.start(
        r#"
import { step, codemod } from "codemod/workflow";
export default async function main() {
  await step(codemod("./child"));
}
"#,
        Value::Null,
    );
    output(&report).unwrap();
    assert_eq!(
        git(&h, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "codemod-child"
    );
}

#[test]
fn shard_steps_can_be_disabled() {
    let h = Harness::with_services(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: false }),
        Services {
            skip_shard_steps: true,
            ..Services::default()
        },
    );
    let report = h.start(
        r#"
import { step, shard } from "codemod/workflow";
export default async function main() {
  return step(shard({ method: { type: "directory", maxFilesPerShard: 1 }, files: "**/*" }));
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([]));
}

#[test]
fn an_approval_the_body_swallows_still_pauses_the_run() {
    let h = Harness::new();
    h.write("a.txt", "a");
    let report = h.start(
        r#"
import { step, approval, forEachFile } from "codemod/workflow";
export default async function main() {
  try {
    await step(approval("Ship it?"));
  } catch {}
  await step(forEachFile("*.txt", (file) => { file.write("shipped"); }));
  return "done";
}
"#,
        Value::Null,
    );
    assert_eq!(report.ending, Ending::AwaitingApproval("Ship it?".into()));
    assert_eq!(h.read("a.txt"), "a", "nothing runs past the gate");
}

#[cfg(unix)]
#[test]
fn ast_grep_does_not_scan_through_a_symlink_outside_the_target() {
    let h = Harness::new();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("a.ts"), "const a = 1;\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), h.target.path().join("link")).unwrap();
    let report = h.start(
        r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
export default async function main() {
  const result = await step(forEachFile("link/**", astGrep({ id: "r", language: "typescript", rule: { pattern: "const a = 1" }, fix: "const a = 2" })));
  return result.changes.written;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([]));
    assert_eq!(
        std::fs::read_to_string(outside.path().join("a.ts")).unwrap(),
        "const a = 1;\n"
    );
}

#[test]
fn renames_are_compared_by_the_file_they_name() {
    let h = Harness::new();
    h.write("a.txt", "a");
    h.write("b.txt", "b");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  // Renaming a file to its own path, spelled differently, changes nothing.
  const same = await step("Same", forEachFile("a.txt", (file) => { file.rename("./a.txt"); }));
  return same.changes;
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "written": [], "renamed": [], "deleted": [] })
    );

    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("Collide", forEachFile("*.txt", (file) => {
    file.rename(file.path === "a.txt" ? "./c.txt" : "c.txt");
  }));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("both renamed to 'c.txt'"), "{error}");
    assert!(h.exists("a.txt") && h.exists("b.txt") && !h.exists("c.txt"));
}

#[test]
fn a_function_step_commits_what_it_changes_with_jssg_transform() {
    let h = Harness::new();
    h.write("a.ts", "const a = 1;\n");
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { jssgTransform } from "codemod/ast-grep";
export default async function main({ target }) {
  await step("Edit", async () => {
    await jssgTransform((root) => root.root().text().replace("1", "2"), `${target}/a.ts`, "typescript");
    await jssgTransform((root) => root.root().text().replace("2", "3"), `${target}/a.ts`, "typescript");
  });
}
"#,
        Value::Null,
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("a.ts"), "const a = 3;\n");
    assert_eq!(h.engine_summary_diffs(), 1);
}

#[test]
fn for_each_file_applies_ast_grep_rules_to_the_files_its_selector_picks() {
    let h = Harness::new();
    h.write("src/a.js", "var x = 1;\n");
    // Files of no rule's language are skipped, not failures.
    h.write("src/README.md", "var x = 1;\n");
    h.write("lib/b.js", "var y = 2;\n");
    h.write("lib/c.js", "var z = 3; // keep\n");
    h.write("lib/d.js", "var w = 4; // keep\n");
    h.bundle_file(
        "rules.yaml",
        "id: no-var\nlanguage: javascript\nrule:\n  pattern: var $A = $B\nfix: let $A = $B;\n",
    );
    let report = h.start(
        r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
import rules from "./rules.yaml";
export default async function main() {
  const some = await step("Some", forEachFile("src/**", astGrep(rules)));
  const kept = await step("Kept", forEachFile({ include: "lib/**", exclude: "lib/d.js", containing: "keep" }, astGrep(rules)));
  const all = await step("All", forEachFile(astGrep(rules)));
  return [some, kept.changes.written, all.changes.written];
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!([
            {
                "changes": { "written": ["src/a.js"], "renamed": [], "deleted": [] },
                "matches": 1,
                "failures": [],
            },
            ["lib/c.js"],
            ["lib/b.js", "lib/d.js"],
        ])
    );
    assert_eq!(h.read("src/a.js"), "let x = 1;\n");
    assert_eq!(h.read("src/README.md"), "var x = 1;\n");
}

#[test]
fn for_each_file_refuses_what_a_transform_cannot_take() {
    let h = Harness::new();
    h.bundle_file(
        "rules.yaml",
        "id: r\nlanguage: javascript\nrule:\n  pattern: a\n",
    );
    let failed = |call: &str| {
        let source = format!(
            "import {{ step, forEachFile, astGrep, shell }} from \"codemod/workflow\";\nimport rules from \"./rules.yaml\";\nexport default async function main() {{ await step({call}); }}\n"
        );
        output(&h.start(&source, Value::Null)).unwrap_err()
    };
    for (call, message) in [
        (
            r#"forEachFile({ include: "a/**" }, astGrep(rules, { include: "b/**" }))"#,
            "astGrep: 'include' goes in forEachFile's selector: forEachFile({ include }, astGrep(rules))",
        ),
        (
            r#"forEachFile(astGrep(rules, { repeat: 0 }))"#,
            "astGrep: repeat must be true (up to 10 passes) or the most passes to make",
        ),
        (
            r#"forEachFile(astGrep(rules, { again: true }))"#,
            "astGrep: unknown option 'again' (it takes repeat)",
        ),
        (
            r#"astGrep(rules)"#,
            "astGrep(...) is not a step on its own; run it on files with step(forEachFile(selector, astGrep(rules)))",
        ),
        (
            r#"forEachFile({ concurrency: 2 }, astGrep(rules))"#,
            "selector.concurrency is for callbacks, not for astGrep(...)",
        ),
        (
            r#"forEachFile({ semanticAnalysis: "none" }, astGrep(rules))"#,
            "selector.semanticAnalysis is for callbacks, not for astGrep(...)",
        ),
        (
            r#"forEachFile({ language: "tsx" }, astGrep(rules))"#,
            "the rules of astGrep(...) name their languages",
        ),
        (
            r#"forEachFile(shell("true"))"#,
            "forEachFile takes a callback, written inline as a function or imported from its file, or astGrep(...)",
        ),
    ] {
        let error = failed(call);
        assert!(error.contains(message), "{call}: {error}");
    }
}

#[test]
fn for_each_file_selects_hidden_files_and_languages() {
    let h = Harness::new();
    // Hidden directories are searched, and a language means the same files
    // for both tasks.
    h.write(".github/workflows/ci.yml", "on: push\n");
    h.write("src/a.ts", "export {};\n");
    h.write("src/b.kt", "fun main() {}\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const hidden = await step("Hidden", forEachFile(".github/**/*.yml", (file) => file.path));
  const kotlin = await step("Kotlin", forEachFile({ language: "kotlin" }, (file) => file.path));
  return { hidden: hidden.outputs, kotlin: kotlin.outputs };
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "hidden": [".github/workflows/ci.yml"], "kotlin": ["src/b.kt"] })
    );
}

const SOME_FILES_FAIL: &str = r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const some = await step("Some", forEachFile("*.ts", (file) => {
    if (file.path === "a.ts") throw new Error("cannot handle a");
    file.write(file.text.trim() + "x\n");
  }));
  await step("Next", () => { throw new Error("not yet"); });
  return some.failures.map((failure) => failure.path);
}
"#;

#[test]
fn a_for_each_file_step_is_partial_when_some_files_fail() {
    let h = Harness::new();
    h.write("a.ts", "a\n");
    h.write("b.ts", "b\n");
    let first = h.start(SOME_FILES_FAIL, Value::Null);

    // The step finished with what the other files did, and the run went on.
    let some = &first.steps[0];
    assert_eq!(some.status, StepStatus::Partial);
    assert_eq!(some.error.as_deref(), Some("1 of 2 files failed"));
    assert_eq!(h.read("b.ts"), "bx\n");
    assert_eq!(first.steps[1].label, "Next");
    assert_eq!(first.steps.len(), 2);
    let events = h.events.take();
    assert!(events.iter().any(|event| matches!(
        event,
        Event::StepFinished { label, status: StepStatus::Partial, error: Some(error), .. }
            if label == "Some" && error == "1 of 2 files failed"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::StepLog { line, .. } if line == "a.ts"
    )));

    // A resume reuses the partial step instead of running it again.
    let second = h.resume(
        &first,
        Some(&SOME_FILES_FAIL.replace("throw new Error(\"not yet\");", "")),
    );
    assert_eq!(output(&second).unwrap(), json!(["a.ts"]));
    assert_eq!(h.read("b.ts"), "bx\n");
}

/// A run that finishes with failed files says how many, in its report and
/// in `run_finished`, also when a resume reused the partial step.
#[test]
fn a_run_with_failed_files_counts_them_when_it_finishes() {
    let h = Harness::new();
    h.write("a.ts", "a\n");
    h.write("b.ts", "b\n");
    h.write("c.ts", "c\n");
    let script = r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("Some", forEachFile("*.ts", (file) => {
    if (file.path !== "b.ts") throw new Error("cannot handle it");
    file.write("bx\n");
  }));
  await step("Next", () => { throw new Error("not yet"); });
}
"#;
    let first = h.start(script, Value::Null);
    assert_eq!(
        first.steps[0].files,
        Some(FileCount {
            failed: 2,
            total: 3
        })
    );
    assert_eq!(first.failed_files(), 2);
    let events = h.events.take();
    let Some(Event::RunFinished {
        status,
        failed_files,
        partial_steps,
        ..
    }) = events.last()
    else {
        panic!("{events:#?}");
    };
    assert_eq!(*status, RunStatus::Failed);
    assert_eq!(*failed_files, 2);
    assert_eq!(
        partial_steps,
        &vec![PartialStep {
            position: 0,
            label: "Some".into(),
            failed_files: 2,
            files: 3,
        }]
    );

    let second = h.resume(
        &first,
        Some(&script.replace("throw new Error(\"not yet\");", "")),
    );
    assert!(output(&second).is_ok(), "{:?}", second.ending);
    assert_eq!(second.failed_files(), 2);
    let events = h.events.take();
    assert!(
        matches!(
            events.last(),
            Some(Event::RunFinished {
                status: RunStatus::Succeeded,
                failed_files: 2,
                ..
            })
        ),
        "{events:#?}"
    );
}

#[test]
fn a_for_each_file_step_fails_when_every_file_fails() {
    let h = Harness::new();
    h.write("a.ts", "a\n");
    h.write("b.ts", "b\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("All", forEachFile("*.ts", (file) => { throw new Error(`cannot handle ${file.path}`); }));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("the callback failed on every file; first error: a.ts:"),
        "{error}"
    );
    assert_eq!(report.steps[0].status, StepStatus::Failed);
}

/// Files that are not text, or lack the `containing` text, are not the
/// step's: the callback does not see them and they do not count.
#[test]
fn for_each_file_leaves_out_files_that_are_not_text_or_lack_the_containing_text() {
    let h = Harness::new();
    h.write("a.ts", "needle\n");
    h.write("b.ts", "hay\n");
    std::fs::write(h.target.path().join("c.ts"), [0xff, 0xfe, b'n', 0xff]).unwrap();
    let workflow = |callback: &str| {
        format!(
            r#"
import {{ step, forEachFile }} from "codemod/workflow";
export default async function main() {{
  const all = await step("All", forEachFile("*.ts", (file) => file.path));
  const some = await step("Some", forEachFile({{ include: "*.ts", containing: "needle" }}, {callback}));
  return {{ all: all.outputs, some: some.outputs }};
}}
"#
        )
    };
    let report = h.start(&workflow("(file) => file.path"), Value::Null);
    assert_eq!(
        output(&report).unwrap(),
        json!({ "all": ["a.ts", "b.ts"], "some": ["a.ts"] })
    );

    // The one file the selector keeps failed: that is every file.
    let report = h.start(
        &workflow(r#"(file) => { throw new Error("no"); }"#),
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("the callback failed on every file; first error: a.ts: Error: no"),
        "{error}"
    );
}

/// A file is read from where the walk found it, not from its `/`-path: on
/// Unix a `\` in a name is not a separator, so `a\..\b.ts` is its own file,
/// not `b.ts`.
#[cfg(unix)]
#[test]
fn for_each_file_reads_a_file_whose_name_has_a_backslash() {
    let h = Harness::new();
    std::fs::create_dir(h.target.path().join("a")).unwrap();
    h.write("b.ts", "b\n");
    h.write("a\\..\\b.ts", "backslash\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  return step(forEachFile("*.ts", (file) => [file.path, file.text]));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap()["outputs"],
        json!([["a/../b.ts", "backslash\n"], ["b.ts", "b\n"]])
    );
}

/// Writing a file its own text is leaving it alone: it is not part of the
/// commit, so a change made to it meanwhile is no conflict.
#[test]
fn a_callback_that_writes_a_file_its_own_text_does_not_change_it() {
    let h = Harness::with_services(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: false }),
        Services {
            capabilities: [codemod_llrt_capabilities::types::LlrtSupportedModules::Fs].into(),
            ..Services::default()
        },
    );
    h.write("a.ts", "a\n");
    h.write("b.ts", "b\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main({ target }) {
  return step(forEachFile({ include: "*.ts", capabilities: ["fs"] }, async (file) => {
    if (file.path === "a.ts") return file.write("A\n");
    const fs = await import("fs");
    fs.writeFileSync(`${target}/b.ts`, "changed meanwhile\n");
    file.write(file.text);
  }));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap()["changes"],
        json!({ "written": ["a.ts"], "renamed": [], "deleted": [] })
    );
    assert_eq!(h.read("a.ts"), "A\n");
    assert_eq!(h.read("b.ts"), "changed meanwhile\n");
    assert_eq!(h.engine_summary_diffs(), 1);
}

#[test]
fn a_workflow_in_src_finds_its_files_and_its_package() {
    let h = Harness::new();
    h.bundle_file(
        "codemod.yaml",
        "schema_version: \"1.0\"\nname: p\nversion: 0.1.0\ntargets:\n  languages: [\"python\"]\n",
    );
    h.bundle_file("src/scripts/echo.sh", "printf '%s|' \"$@\"\n");
    h.write("a.py", "x = 1\n");
    h.write("b.ts", "x\n");
    let workflow = h.workflow_file(
        "src/workflow.ts",
        r#"
import { step, shell, forEachFile } from "codemod/workflow";
const here = import.meta.dirname;
export default async function main() {
  const echoed = await step(shell`sh ${import.meta.dirname}/scripts/echo.sh ${"two words"} ${["it's", 3]}`);
  // No selector: the package's languages, from the codemod.yaml above src/.
  const seen = await step(forEachFile((file) => ({
    path: file.path,
    dirname: import.meta.dirname,
    filename: import.meta.filename,
  })));
  return { here, url: import.meta.url, echoed: echoed.stdout, seen: seen.outputs };
}
"#,
    );
    let report = h.start_loaded(workflow, Value::Null, false);
    let result = output(&report).unwrap();

    let src = std::fs::canonicalize(h.bundle.path().join("src")).unwrap();
    let src = src.to_string_lossy();
    assert_eq!(result["here"], json!(src));
    assert_eq!(result["url"], json!(format!("file://{src}/workflow.ts")));
    assert!(
        result["echoed"]
            .as_str()
            .unwrap()
            .contains("two words|it's|3|"),
        "{result}"
    );
    assert_eq!(
        result["seen"],
        json!([{ "path": "a.py", "dirname": src, "filename": format!("{src}/workflow.ts") }])
    );
}

const YARN_LOCK: &str = r#"
"app@workspace:.":
  version: 0.0.0-use.local
  resolution: "app@workspace:."

"table@npm:^3.0.0":
  version: 3.2.5
  resolution: "table@npm:3.2.5"
  dependencies:
    dnd: "npm:^13.0.0"

"dnd@npm:^13.0.0":
  version: 13.0.0
  resolution: "dnd@npm:13.0.0"
  peerDependencies:
    react: ^16.8.5 || ^17.0.0 || ^18.0.0
"#;

fn yarn_target(h: &Harness) {
    h.write("yarn.lock", YARN_LOCK);
    h.write(
        "package.json",
        r#"{ "name": "app", "dependencies": { "table": "^3.0.0" } }"#,
    );
}

/// The demo monorepo: web -> @demo/shared -> moment, api -> moment.
fn npm_workspaces_target(h: &Harness) {
    h.write(
        "package-lock.json",
        include_str!("../../deps/tests/fixtures/npm-workspaces/package-lock.json"),
    );
}

#[test]
fn who_uses_a_package_follows_workspace_to_workspace_links() {
    let h = Harness::new();
    npm_workspaces_target(&h);
    // The "who uses a package" example in docs/community/workflows/reference.mdx.
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step(() => ((g) => {
    const installed = g.nodes("moment");
    return {
      installed: installed.map((node) => node.version),
      declaredBy: g.dependents("moment")
        .map((edge) => ({ dir: edge.from.dir, range: edge.range, kind: edge.kind })),
      reachedBy: g.chains({ to: installed })
        .filter((chain) => chain.length > 2)
        .map((chain) => ({ dir: chain[0].dir, via: chain.slice(1, -1).map((node) => node.name) })),
    };
  })(dependencies()));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({
            "installed": ["2.29.4"],
            "declaredBy": [
                { "dir": "packages/api", "range": "^2.29.4", "kind": "prod" },
                { "dir": "packages/shared", "range": "^2.29.4", "kind": "prod" },
            ],
            "reachedBy": [{ "dir": "packages/web", "via": ["@demo/shared"] }],
        })
    );
}

#[test]
fn a_node_knows_its_directory_and_what_installed_it() {
    let h = Harness::new();
    npm_workspaces_target(&h);
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step(() => ((g) => {
    const shared = g.node("@demo/shared");
    return {
      shared: [shared.dir, shared.local],
      moment: [g.node("moment").dir, g.node("moment").local],
      installedBy: shared.installedBy().map((edge) => edge.from.dir),
    };
  })(dependencies()));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({
            "shared": ["packages/shared", true],
            "moment": [null, false],
            "installedBy": ["packages/api", "packages/web"],
        })
    );
}

#[test]
fn dependents_are_the_repositorys_own_packages_unless_third_party_ones_are_asked_for() {
    let h = Harness::new();
    npm_workspaces_target(&h);
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step(() => ((g) => ({
    shared: g.dependents("@demo/shared").map((e) => e.from.dir),
    undici: g.dependents("undici-types").map((e) => e.from),
    undiciAll: g.dependents("undici-types", { includeThirdParty: true }).map((e) => e.from),
    installedBy: g.node("undici-types").installedBy().map((e) => e.from),
    installedByAll: g.node("undici-types").installedBy({ includeThirdParty: true, kinds: "prod" }).map((e) => e.from),
    // The root package dev-depends on @types/node; codemod optionally depends on its binaries.
    typesNode: g.node("@types/node").installedBy().map((e) => e.kind),
    declaresTypesNode: g.dependents("@types/node").map((e) => [e.from.dir, e.kind]),
    declaresTypesNodeProd: g.dependents("@types/node", { kinds: "prod" }).map((e) => e.from.dir),
    typesNodeProd: g.node("@types/node").installedBy({ kinds: "prod" }).map((e) => e.from),
    binary: g.node("@codemod.com/cli-linux-x64-gnu").installedBy({ includeThirdParty: true }).map((e) => e.kind),
    binaryProd: g.node("@codemod.com/cli-linux-x64-gnu").installedBy({ includeThirdParty: true, kinds: "prod" }).map((e) => e.from),
  }))(dependencies()));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({
            "shared": ["packages/api", "packages/web"],
            "undici": [],
            "undiciAll": ["@types/node@22.20.5"],
            "installedBy": [],
            "installedByAll": ["@types/node@22.20.5"],
            "typesNode": ["dev"],
            "declaresTypesNode": [[".", "dev"]],
            "declaresTypesNodeProd": [],
            "typesNodeProd": [],
            "binary": ["optional"],
            "binaryProd": [],
        })
    );
}

#[test]
fn node_by_name_alone_fails_when_several_versions_are_installed() {
    let h = Harness::new();
    h.write(
        "package-lock.json",
        r#"{
          "lockfileVersion": 3,
          "packages": {
            "": { "name": "app", "dependencies": { "a": "^1.0.0", "b": "^2.0.0" } },
            "node_modules/a": { "version": "1.0.0", "dependencies": { "b": "^1.0.0" } },
            "node_modules/a/node_modules/b": { "version": "1.5.0" },
            "node_modules/b": { "version": "2.1.0" }
          }
        }"#,
    );
    let workflow = |body: &str| {
        format!(
            r#"
import {{ step }} from "codemod/workflow";
import {{ dependencies }} from "codemod/deps";
export default async function main() {{ return await step("B", () => ((g) => {body})(dependencies())); }}
"#
        )
    };
    let report = h.start(&workflow(r#"g.node("b", "1.5.0")"#), Value::Null);
    assert_eq!(output(&report).unwrap(), json!("b@1.5.0"));
    let report = h.start(&workflow(r#"g.node("b", "2.1.0")"#), Value::Null);
    assert_eq!(output(&report).unwrap(), json!("b@2.1.0"));
    let report = h.start(&workflow(r#"g.node("b")"#), Value::Null);
    let error = output(&report).unwrap_err();
    assert!(
        error.contains(r#"node("b"): 2 versions are installed (1.5.0, 2.1.0); use nodes("b") or pass a version"#),
        "{error}"
    );
}

/// dnd's releases, counting how often the registry is asked.
#[derive(Default)]
struct FakeRegistry(std::sync::atomic::AtomicUsize);

impl codemod_deps::Registry for FakeRegistry {
    fn releases(
        &self,
        _: codemod_deps::Ecosystem,
        names: &[String],
        _: &std::sync::atomic::AtomicBool,
    ) -> Result<std::collections::BTreeMap<String, Vec<codemod_deps::Release>>, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let release = |version: &str, react: &str| codemod_deps::Release {
            version: version.into(),
            yanked: false,
            dependencies: vec![("react".into(), react.into(), codemod_deps::Kind::Peer)],
        };
        let dnd = vec![
            release("13.0.0", "^16.8.5 || ^17.0.0 || ^18.0.0"),
            release("13.1.0", "^16.8.5 || ^17.0.0 || ^18.0.0"),
            release("14.0.0", "^18.0.0 || ^19.0.0"),
        ];
        Ok(names
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    if name == "dnd" {
                        dnd.clone()
                    } else {
                        Vec::new()
                    },
                )
            })
            .collect())
    }
}

fn with_registry(registry: Arc<FakeRegistry>) -> Harness {
    Harness::with_services(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: false }),
        Services {
            package_registry: Some(registry),
            ..Services::default()
        },
    )
}

#[test]
fn a_dependency_graph_callback_walks_the_targets_lockfiles_through_a_live_handle() {
    let h = Harness::new();
    yarn_target(&h);
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step(() => ((g) => g.chains({ to: g.node("dnd") }))(dependencies()));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!([["app@0.0.0-use.local", "table@3.2.5", "dnd@13.0.0"]])
    );
}

#[test]
fn node_releases_keep_newer_versions_whose_ranges_accept_the_asked_version() {
    let h = with_registry(Arc::default());
    yarn_target(&h);
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step(() => (async (g) => {
    const releases = await g.node("dnd").releases({ accepting: { react: "19.0.0" }, newerThanMine: true });
    return releases.map((r) => `${r.version}${r.breaking ? " breaking" : ""}`);
  })(dependencies()));
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["14.0.0 breaking"]));
}

#[test]
fn a_resumed_run_replays_a_dependency_graph_step_without_asking_the_registry_again() {
    let registry = Arc::new(FakeRegistry::default());
    let h = with_registry(registry.clone());
    yarn_target(&h);
    let workflow = |later: &str| {
        format!(
            r#"
import {{ step }} from "codemod/workflow";
import {{ dependencies, registry }} from "codemod/deps";
export default async function main() {{
  const dnd = await step(() => (async () => (await registry.npm("dnd")).versions.length)(dependencies()));
  await step("Later", () => {{ {later} }});
  return dnd;
}}
"#
        )
    };
    let failed = h.start(&workflow(r#"throw new Error("not yet");"#), Value::Null);
    h.resume(&failed, Some(&workflow("return 1;")));
    assert_eq!(registry.0.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn a_for_each_file_callback_can_await_a_registry_lookup() {
    let h = with_registry(Arc::default());
    h.write("a.txt", "dnd");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { registry } from "codemod/deps";
export default async function main() {
  const found = await step(forEachFile("*.txt", async (file) => (await registry.npm(file.text)).versions.length));
  return found.outputs;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([3]));
}

#[test]
fn a_function_step_can_await_a_registry_lookup() {
    let h = with_registry(Arc::default());
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { registry } from "codemod/deps";
export default async function main() {
  return await step("Look up dnd", async () => (await registry.npm("dnd")).versions.length);
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(3));
}

#[test]
fn a_registry_lookup_outside_a_step_fails_because_it_would_not_be_recorded() {
    let h = with_registry(Arc::default());
    let report = h.start(
        r#"
import { registry } from "codemod/deps";
export default async function main() { await registry.npm("dnd"); }
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("must run inside a step"), "{error}");
}

#[test]
fn a_dependency_graph_of_a_target_without_lockfiles_is_empty() {
    let h = Harness::new();
    h.write("package.json", r#"{ "name": "app" }"#);
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step("Deps", () => ((g) => ({ read: g.lockfiles, skipped: g.skipped, nodes: g.nodes().length }))(dependencies()));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "read": [], "skipped": [], "nodes": 0 })
    );
}

#[test]
fn a_dependency_graph_step_fails_when_its_lockfiles_glob_matches_none() {
    let h = Harness::new();
    yarn_target(&h);
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() { await step("Deps", () => ((g) => g.workspaces())(dependencies({ lockfiles: "**/pnpm-lock.yaml" }))); }
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("step 'Deps' failed: no lockfile"), "{error}");
}

#[test]
fn a_dependency_graph_whose_only_lockfile_cannot_be_read_lists_it_as_skipped() {
    let h = Harness::new();
    h.write("package.json", r#"{ "name": "app" }"#);
    h.write("pnpm-lock.yaml", "lockfileVersion: [\n");
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step(() => ((g) => ({ read: g.lockfiles, skipped: g.skipped.map((s) => s.lockfile) }))(dependencies()));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "read": [], "skipped": ["pnpm-lock.yaml"] })
    );
}

#[test]
fn a_dependency_graph_reads_a_yarn_1_lockfile() {
    let h = Harness::new();
    h.write(
        "package.json",
        r#"{ "name": "app", "dependencies": { "left-pad": "^1.3.0" } }"#,
    );
    h.write(
        "yarn.lock",
        "# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.\n# yarn lockfile v1\n\n\nleft-pad@^1.3.0:\n  version \"1.3.0\"\n  resolved \"https://registry.yarnpkg.com/left-pad/-/left-pad-1.3.0.tgz\"\n",
    );
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step(() => ((g) => ({ read: g.lockfiles, leftPad: g.dependents("left-pad").map((e) => [e.from.name, e.range, e.to.version]) }))(dependencies()));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "read": ["yarn.lock"], "leftPad": [["app", "^1.3.0", "1.3.0"]] })
    );
}

#[test]
fn a_dependency_graph_callback_commits_what_it_changes_with_jssg_transform() {
    let h = Harness::new();
    yarn_target(&h);
    h.write("a.ts", "const version = 0;\n");
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
import { jssgTransform } from "codemod/ast-grep";
export default async function main({ target }) {
  await step(() => (async (g) => {
    const dnd = g.node("dnd").version;
    await jssgTransform((root) => root.root().text().replace("0", JSON.stringify(dnd)), `${target}/a.ts`, "typescript");
  })(dependencies()));
}
"#,
        Value::Null,
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("a.ts"), "const version = \"13.0.0\";\n");
}

#[test]
fn a_dependency_graph_reads_only_root_lockfiles_and_lists_ones_it_could_not_read() {
    let h = Harness::new();
    yarn_target(&h);
    h.write("pnpm-lock.yaml", "lockfileVersion: [\n");
    h.write(
        "examples/old/yarn.lock",
        "# yarn lockfile v1\n\na@^1:\n  version \"1.0.0\"\n",
    );
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step(() => ((g) => ({ read: g.lockfiles, skipped: g.skipped.map((s) => [s.lockfile, typeof s.error === "string" && s.error.length > 0]) }))(dependencies()));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "read": ["yarn.lock"], "skipped": [["pnpm-lock.yaml", true]] })
    );
}

#[test]
fn a_dependency_graph_reads_the_lockfiles_its_options_select() {
    let h = Harness::new();
    // The root lockfile has moment; the selected nested one has table instead.
    npm_workspaces_target(&h);
    h.write("examples/app/yarn.lock", YARN_LOCK);
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step(() => ((g) => ({
    read: g.lockfiles,
    table: g.nodes("table").length > 0,
    moment: g.nodes("moment").length,
  }))(dependencies({ lockfiles: "examples/app/yarn.lock" })));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "read": ["examples/app/yarn.lock"], "table": true, "moment": 0 })
    );
}

#[test]
fn a_helper_file_a_step_imports_can_use_registry_and_semver() {
    let h = with_registry(Arc::default());
    h.bundle_file(
        "lib/releases.ts",
        r#"
import { registry, semver } from "codemod/deps";
import { step } from "codemod/workflow";
export async function newest(name: string) {
  const { versions } = await registry.npm(name);
  return versions.map((r) => r.version).sort((a, b) => semver.compare(a, b)).at(-1);
}
// Shared with the workflow body: importing body-only functions still works.
export const run = (name: string) => step(name, async () => newest(name));
"#,
    );
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { newest } from "./lib/releases.ts";
export default async function main() {
  return await step("Newest dnd", async () => newest("dnd"));
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!("14.0.0"));
}

#[test]
fn a_body_only_function_called_inside_a_step_says_so() {
    let h = Harness::new();
    h.bundle_file(
        "lib/nested.ts",
        r#"
import { step, shell } from "codemod/workflow";
export const nested = () => step("Inner", shell("true"));
"#,
    );
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { nested } from "./lib/nested.ts";
export default async function main() {
  await step("Outer", async () => nested());
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("shell() can only be called in the workflow body"),
        "{error}"
    );
}

/// A registry that answers only when its lookup is canceled.
struct HangingRegistry;

impl codemod_deps::Registry for HangingRegistry {
    fn releases(
        &self,
        _: codemod_deps::Ecosystem,
        _: &[String],
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<std::collections::BTreeMap<String, Vec<codemod_deps::Release>>, String> {
        let started = std::time::Instant::now();
        while !cancel.load(std::sync::atomic::Ordering::SeqCst) {
            if started.elapsed() > std::time::Duration::from_secs(20) {
                return Err("never canceled".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Err("the registry lookup was canceled".into())
    }
}

#[test]
fn a_step_that_runs_out_of_time_stops_its_registry_lookup() {
    let h = Harness::with_services(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: false }),
        Services {
            package_registry: Some(Arc::new(HangingRegistry)),
            ..Services::default()
        },
    );
    let started = std::time::Instant::now();
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { registry } from "codemod/deps";
export default async function main() {
  await step("Look up", async () => (await registry.npm("dnd")).versions.length, { timeout: "300ms" });
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("timed out"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn a_dependency_graph_result_keeps_a_proto_key_as_data() {
    let h = Harness::new();
    yarn_target(&h);
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step(() => (() => JSON.parse('{"__proto__": {"a": 1}, "b": 2}'))(dependencies()));
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "__proto__": { "a": 1 }, "b": 2 })
    );
}

const NO_VAR: &str = r#"{ id: "no-var", language: "javascript", rule: { pattern: "var $A = $B" }, fix: "let $A = $B;" }"#;

#[test]
fn ast_grep_takes_a_rule_written_in_the_workflow() {
    let h = Harness::new();
    h.write("a.js", "var x = 1;\n");
    let report = h.start(
        &format!(
            r#"
import {{ step, forEachFile, astGrep }} from "codemod/workflow";
export default async function main() {{
  const result = await step(forEachFile(astGrep({NO_VAR})));
  return result.changes.written;
}}
"#
        ),
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["a.js"]));
    assert_eq!(h.read("a.js"), "let x = 1;\n");
    // Labeled by the rules' ids.
    assert_eq!(report.steps[0].label, "astGrep no-var");
}

#[test]
fn ast_grep_takes_rules_built_at_run_time_and_several_at_once() {
    let h = Harness::new();
    h.write("a.js", "var x = 1;\nold();\n");
    let report = h.start(
        &format!(
            r#"
import {{ step, forEachFile, astGrep }} from "codemod/workflow";
export default async function main({{ params }}) {{
  const rename = {{
    id: "rename",
    language: "javascript",
    rule: {{ pattern: `${{params.from}}()` }},
    fix: `${{params.to}}()`,
    message: undefined,
  }};
  const result = await step(forEachFile(astGrep([{NO_VAR}, rename])));
  return result.matches;
}}
"#
        ),
        json!({ "from": "old", "to": "fresh" }),
    );
    assert_eq!(output(&report).unwrap(), json!(2));
    assert_eq!(h.read("a.js"), "let x = 1;\nfresh();\n");
}

/// Nested matches (one rule's match inside another's) keep the outer fix
/// instead of splicing both into the file; every match is counted.
#[test]
fn ast_grep_keeps_the_outer_fix_of_nested_matches() {
    let h = Harness::new();
    h.write("a.js", "console.log(console.log(1));\n");
    let report = h.start(
        r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
export default async function main() {
  const result = await step(forEachFile(astGrep({ id: "log", language: "javascript", rule: { pattern: "console.log($A)" }, fix: "logger.info($A)" })));
  return result.matches;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(2));
    assert_eq!(h.read("a.js"), "logger.info(console.log(1));\n");
}

/// A `typescript` rule selects `.js` files too, and applies to them: they are
/// parsed as TypeScript, as `forEachFile({ language: "typescript" }, ...)`
/// callbacks parse them.
#[test]
fn a_typescript_rule_applies_to_the_js_files_it_selects() {
    let h = Harness::new();
    for name in ["a.js", "b.ts", "c.mjs", "d.cjs", "e.py"] {
        h.write(name, "console.log(1);\n");
    }
    let report = h.start(
        r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
export default async function main() {
  const result = await step(forEachFile(astGrep({ id: "x", language: "typescript", rule: { pattern: "console.log($A)" }, fix: "logger.info($A)" })));
  return result.changes.written;
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!(["a.js", "b.ts", "c.mjs", "d.cjs"])
    );
    for name in ["a.js", "b.ts", "c.mjs", "d.cjs"] {
        assert_eq!(h.read(name), "logger.info(1);\n", "{name}");
    }
    assert_eq!(h.read("e.py"), "console.log(1);\n");
}

/// Rules of several languages each run on the files of their language,
/// parsed with it: a `.js` file both a `javascript` and a `typescript` rule
/// select gets both fixes, and a `.py` file only the `python` one.
#[test]
fn ast_grep_rules_of_several_languages_each_apply_to_their_files() {
    for repeat in ["", ", { repeat: true }"] {
        let h = Harness::new();
        h.write("a.js", "console.log(1);\nalert(2);\n");
        h.write("b.ts", "console.log(1);\nalert(2);\n");
        h.write("c.py", "print(1)\n");
        let report = h.start(
            &format!(
                r#"
import {{ step, forEachFile, astGrep }} from "codemod/workflow";
export default async function main() {{
  const result = await step(forEachFile(astGrep([
    {{ id: "log", language: "typescript", rule: {{ pattern: "console.log($A)" }}, fix: "logger.info($A)" }},
    {{ id: "alert", language: "javascript", rule: {{ pattern: "alert($A)" }}, fix: "notify($A)" }},
    {{ id: "print", language: "python", rule: {{ pattern: "print($A)" }}, fix: "log($A)" }},
  ]{repeat})));
  return [result.changes.written, result.matches];
}}
"#
            ),
            Value::Null,
        );
        // a.js gets a typescript and a javascript fix, b.ts and c.py one each.
        assert_eq!(
            output(&report).unwrap(),
            json!([["a.js", "b.ts", "c.py"], 4]),
            "{repeat}"
        );
        assert_eq!(h.read("a.js"), "logger.info(1);\nnotify(2);\n");
        assert_eq!(h.read("b.ts"), "logger.info(1);\nalert(2);\n");
        assert_eq!(h.read("c.py"), "log(1)\n");
    }
}

/// Rules of two languages that undo each other on a file fail it, rather
/// than counting fixes that leave nothing changed.
#[test]
fn ast_grep_rules_of_two_languages_that_undo_each_other_fail_the_file() {
    let h = Harness::new();
    h.write("a.js", "a();\n");
    let report = h.start(
        r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
export default async function main() {
  await step(forEachFile(astGrep([
    { id: "ab", language: "typescript", rule: { pattern: "a()" }, fix: "b()" },
    { id: "ba", language: "javascript", rule: { pattern: "b()" }, fix: "a()" },
  ], { repeat: true })));
}
"#,
        Value::Null,
    );
    // Its only file failed, so the step did.
    let error = output(&report).unwrap_err();
    assert!(error.contains("undo each other"), "{error}");
    assert_eq!(h.read("a.js"), "a();\n");
}

/// In a dry run, a file can't be moved under a path an earlier step moved a
/// file to, as a real run can't.
#[test]
fn a_dry_run_refuses_a_move_under_a_file_an_earlier_step_made() {
    let source = r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("One", forEachFile("a.txt", (file) => file.rename("out")));
  await step("Two", forEachFile("b.txt", (file) => file.rename("out/b.txt")));
}
"#;
    for dry in [false, true] {
        let h = Harness::new();
        h.write("a.txt", "a\n");
        h.write("b.txt", "b\n");
        let report = if dry {
            h.dry_run(source)
        } else {
            h.start(source, Value::Null)
        };
        let failed = output(&report).is_err()
            || report
                .steps
                .iter()
                .any(|step| step.label == "Two" && step.status != StepStatus::Succeeded);
        assert!(failed, "dry: {dry}: {:?}", report.ending);
    }
}

/// A function step's `jssgTransform` that only renames a file an earlier
/// dry-run step made moves it.
#[test]
fn a_dry_run_keeps_a_rename_only_jssg_transform_of_a_file_an_earlier_step_made() {
    let h = Harness::new();
    h.write("a.ts", "const a = 1;\n");
    let report = h.dry_run(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { jssgTransform } from "codemod/ast-grep";
export default async function main({ target }) {
  await step("One", forEachFile("a.ts", (file) => file.rename("b.ts")));
  await step("Two", async () => {
    await jssgTransform((root) => { root.rename("c.ts"); return root.root().text(); }, `${target}/b.ts`, "typescript");
  });
}
"#,
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    let paths: Vec<String> = h
        .events
        .take()
        .into_iter()
        .filter_map(|event| match event {
            Event::RunDiff { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    assert_eq!(paths, ["a.ts", "c.ts"]);
}

const CONSOLE_TO_LOGGER: &str = r#"{ id: "log", language: "javascript", rule: { pattern: "console.log($A)" }, fix: "logger.info($A)" }"#;

/// With `repeat`, the rules run again on the text they fixed until it no
/// longer changes: the inner match of a nested pair is fixed too, in one
/// step. `matches` counts each fix once.
#[test]
fn ast_grep_repeats_until_nothing_changes() {
    let h = Harness::new();
    h.write(
        "a.js",
        "console.log(console.log(console.log(1)));
console.log(2);
",
    );
    h.write(
        "b.js",
        "const b = 1;
",
    );
    let report = h.start(
        &format!(
            r#"
import {{ step, forEachFile, astGrep }} from "codemod/workflow";
export default async function main() {{
  const result = await step(forEachFile(astGrep({CONSOLE_TO_LOGGER}, {{ repeat: true }})));
  return [result.matches, result.changes.written];
}}
"#
        ),
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([4, ["a.js"]]));
    assert_eq!(
        h.read("a.js"),
        "logger.info(logger.info(logger.info(1)));\nlogger.info(2);\n"
    );
    assert_eq!(report.steps.len(), 1);
}

/// A dry run repeats on the text in memory, so it reports the diff a real
/// run makes, though nothing is written between passes.
#[test]
fn a_dry_run_repeats_to_the_same_diff_as_a_real_run() {
    let source = format!(
        r#"
import {{ step, forEachFile, astGrep }} from "codemod/workflow";
export default async function main() {{
  const result = await step(forEachFile(astGrep({CONSOLE_TO_LOGGER}, {{ repeat: true }})));
  return result.matches;
}}
"#
    );
    let input = "console.log(console.log(1));\n";
    let diffs = |h: &Harness| -> Vec<(String, String)> {
        h.summary
            .diffs()
            .into_iter()
            .map(|diff| (diff.path, diff.diff_text))
            .collect()
    };
    let dry = Harness::new();
    dry.write("a.js", input);
    let report = dry.dry_run(&source);
    assert_eq!(output(&report).unwrap(), json!(2));
    assert_eq!(dry.read("a.js"), input, "a dry run writes nothing");
    let real = Harness::new();
    real.write("a.js", input);
    let report = real.start(&source, Value::Null);
    assert_eq!(output(&report).unwrap(), json!(2));
    assert_eq!(real.read("a.js"), "logger.info(logger.info(1));\n");
    assert_eq!(diffs(&dry), diffs(&real));
    assert!(
        diffs(&dry)[0].1.contains("+logger.info(logger.info(1));"),
        "{:?}",
        diffs(&dry)
    );
}

/// A fix that matches its own output would repeat forever: after the most
/// passes the file is a failure and is left as it was.
#[test]
fn ast_grep_repeat_stops_at_its_most_passes() {
    let h = Harness::new();
    h.write("a.js", "wrap(1);\n");
    h.write("b.js", "console.log(console.log(1));\n");
    let report = h.start(
        &format!(
            r#"
import {{ step, forEachFile, astGrep }} from "codemod/workflow";
export default async function main() {{
  const grow = {{ id: "grow", language: "javascript", rule: {{ pattern: "wrap($A)" }}, fix: "wrap(wrap($A))" }};
  const result = await step(forEachFile(astGrep([grow, {CONSOLE_TO_LOGGER}], {{ repeat: 3 }})));
  return [result.changes.written, result.failures.map((f) => f.path)];
}}
"#
        ),
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([["b.js"], ["a.js"]]));
    assert_eq!(h.read("a.js"), "wrap(1);\n");
    let failure = report.steps[0].result.as_ref().unwrap()["failures"][0]["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        failure.contains("the rules still change this file after 3 passes"),
        "{failure}"
    );
}

#[test]
fn ast_grep_refuses_rules_that_are_not_json_or_are_paths() {
    let h = Harness::new();
    let failed = |rules: &str| {
        let source = format!(
            "import {{ step, forEachFile, astGrep }} from \"codemod/workflow\";\nconst run = astGrep;\nexport default async function main() {{ await step(forEachFile(run({rules}))); }}\n"
        );
        output(&h.start(&source, Value::Null)).unwrap_err()
    };
    for (rules, message) in [
        (
            r#"{ id: "a", language: "javascript", rule: { regex: /x/ } }"#,
            "astGrep: the rule.rule.regex is a RegExp, which is not JSON",
        ),
        (
            r#"[{ id: "a", language: "javascript", rule: { any: [undefined] } }]"#,
            "astGrep: rule 0.rule.any[0] is undefined, which is not JSON",
        ),
        (
            r#"{ id: "a", language: "javascript", rule: { pattern: "x" }, fix: () => "y" }"#,
            "astGrep: the rule.fix is a function, which is not JSON",
        ),
        (r#"[]"#, "astGrep needs at least one rule"),
        (
            r#"(() => { const r = { id: "a", language: "javascript", rule: { any: [] } }; r.rule.any.push(r.rule); return r; })()"#,
            "astGrep: the rule.rule.any[0].any[0] is a circular reference",
        ),
        (r#"["x"]"#, "astGrep: rule 0 must be an object"),
        (
            r#""rules/config.yml""#,
            "astGrep takes rules, not a file path: import rules from \"./rules/config.yml\", then astGrep(rules)",
        ),
        (
            r#"{ id: "a", language: "javascript", rule: { nonsense: 1 } }"#,
            "ast-grep failed",
        ),
    ] {
        let error = failed(rules);
        assert!(error.contains(message), "{rules}: {error}");
    }
}

#[test]
fn a_changed_ast_grep_rule_runs_its_step_again_on_resume() {
    let h = Harness::new();
    h.write("a.js", "var x = 1;\n");
    let workflow = |fix: &str| {
        format!(
            r#"
import {{ step, forEachFile, astGrep }} from "codemod/workflow";
export default async function main() {{
  const result = await step("Rules", forEachFile(astGrep({{ id: "r", language: "javascript", rule: {{ pattern: "var $A = $B" }}, fix: "{fix} $A = $B;" }})));
  await step("Check", () => {{ throw new Error("not yet"); }});
  return result.matches;
}}
"#
        )
    };
    let report = h.start(&workflow("let"), Value::Null);
    assert!(output(&report).is_err());
    assert_eq!(h.read("a.js"), "let x = 1;\n");

    // Same rule: the step is replayed; the file is not touched again.
    h.write("a.js", "var x = 1;\n");
    let resumed = h.resume(&report, Some(&workflow("let")));
    assert!(output(&resumed).is_err());
    assert_eq!(h.read("a.js"), "var x = 1;\n");

    // A changed rule is a changed step: it runs again.
    let resumed = h.resume(&report, Some(&workflow("const")));
    assert!(output(&resumed).is_err());
    assert_eq!(h.read("a.js"), "const x = 1;\n");
}

#[test]
fn imported_yaml_is_data_in_the_body_and_in_callbacks() {
    let h = Harness::new();
    h.write("a.js", "var x = 1;\n");
    h.bundle_file(
        "rules.yaml",
        "id: one\nlanguage: javascript\nrule:\n  pattern: var $A = $B\nfix: let $A = $B;\n---\nid: two\nlanguage: javascript\nrule:\n  pattern: nothing()\n",
    );
    let report = h.start(
        r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
import rules from "./rules.yaml";
export default async function main() {
  const ids = await step("Ids", forEachFile("a.js", () => rules.map((rule) => rule.id)));
  const applied = await step(forEachFile(astGrep(rules)));
  return { ids: ids.outputs[0], written: applied.changes.written };
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "ids": ["one", "two"], "written": ["a.js"] })
    );
    assert_eq!(h.read("a.js"), "let x = 1;\n");
}

#[test]
fn imported_yaml_keeps_a_proto_key_as_a_key() {
    let h = Harness::new();
    h.bundle_file("data.yaml", "__proto__:\n  enabled: true\nname: x\n");
    let report = h.start(
        r#"
import data from "./data.yaml";
export default async function main() {
  return { keys: Object.keys(data), inherited: (data as any).enabled ?? null, own: JSON.stringify(data) };
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({
            "keys": ["__proto__", "name"],
            "inherited": null,
            "own": r#"{"__proto__":{"enabled":true},"name":"x"}"#,
        })
    );
}

/// A callback's roots resolve definitions and references within their file
/// by default, across the selected files with `semanticAnalysis:
/// "workspace"`, and not at all with `"none"`.
#[test]
fn callbacks_have_semantic_analysis_as_the_selector_says() {
    let h = Harness::new();
    h.write("src/a.ts", "const value = 1;\nconsole.log(value);\n");
    h.write("src/lib.ts", "export function helper() {}\n");
    h.write("src/b.ts", "import { helper } from \"./lib\";\nhelper();\n");
    let workflow = |options: &str| {
        format!(
            r#"
import {{ step, forEachFile }} from "codemod/workflow";
import {{ parse }} from "codemod/ast-grep";
export default async function main() {{
  const found = await step(forEachFile({{ include: "src/**/*.ts", language: "typescript"{options} }}, (file) => {{
    const root = parse(file).root();
    if (file.path === "src/a.ts") {{
      const definition = root.find({{ rule: {{ pattern: "console.log($A)" }} }}).getMatch("A").definition();
      return ["definition", definition ? definition.node.text() : null];
    }}
    if (file.path === "src/lib.ts") {{
      const name = root.find({{ rule: {{ kind: "identifier", inside: {{ kind: "function_declaration" }} }} }});
      const files = name.references().map((found) => found.root.filename().split("/").pop()).sort();
      return ["references", files];
    }}
  }}));
  return found.outputs;
}}
"#
        )
    };
    assert_eq!(
        output(&h.start(&workflow(""), Value::Null)).unwrap(),
        json!([["definition", "value"], ["references", []]])
    );
    assert_eq!(
        output(&h.start(&workflow(r#", semanticAnalysis: "workspace""#), Value::Null)).unwrap(),
        json!([["definition", "value"], ["references", ["b.ts"]]])
    );
    for none in [r#""none""#, r#"{ mode: "none" }"#] {
        assert_eq!(
            output(&h.start(
                &workflow(&format!(", semanticAnalysis: {none}")),
                Value::Null
            ))
            .unwrap(),
            json!([["definition", null], ["references", []]]),
            "{none}"
        );
    }
}

/// A callback gets the capabilities its selector asks for once they are
/// approved, and never `fs` or `child_process` in a dry run.
#[test]
fn callbacks_get_the_capabilities_their_selector_asks_for() {
    let workflow = |options: &str| {
        format!(
            r#"
import {{ step, forEachFile }} from "codemod/workflow";
export default async function main() {{
  const read = await step(forEachFile({{ include: "a.txt"{options} }}, async (file) => {{
    const fs = await import("fs");
    return fs.readFileSync(`${{import.meta.dirname}}/data.txt`, "utf8").trim();
  }}));
  return read;
}}
"#
        )
    };
    let denied = Harness::new();
    denied.write("a.txt", "a\n");
    denied.bundle_file("data.txt", "secret\n");
    let report = denied.start(&workflow(r#", capabilities: ["fs"]"#), Value::Null);
    let error = output(&report).unwrap_err();
    assert!(error.contains("fs"), "{error}");

    // Granted to the run (--allow-fs, or codemod.yaml), it is the callback's.
    let h = Harness::with_services(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: false }),
        Services {
            capabilities: [codemod_llrt_capabilities::types::LlrtSupportedModules::Fs].into(),
            ..Services::default()
        },
    );
    h.write("a.txt", "a\n");
    h.bundle_file("data.txt", "secret\n");
    let report = h.start(&workflow(r#", capabilities: ["fs"]"#), Value::Null);
    assert_eq!(output(&report).unwrap()["outputs"], json!(["secret"]));
    // A dry run installs no file system, whatever was granted.
    let error = output(&h.dry_run(&workflow(r#", capabilities: ["fs"]"#))).unwrap_err();
    assert!(error.contains("fs"), "{error}");

    let error =
        output(&h.start(&workflow(r#", capabilities: ["disk"]"#), Value::Null)).unwrap_err();
    assert!(error.contains("capabilities must be an array"), "{error}");
}

/// A callback imported from a file: its default export, with the inline
/// callback's signature.
const MIGRATE: &str = r#"
import { parse } from "codemod/ast-grep";
import { suffix } from "./helpers.ts";
// The workflow body never evaluates this module.
if ((globalThis as any).__workflowHost !== undefined) throw new Error("loaded by the body");
export default function migrate(file: any) {
  const root = parse(file);
  const found = root.root().findAll({ rule: { pattern: "var $A = $B" } });
  if (file.path.endsWith("old.ts")) file.rename(file.path.replace("old.ts", "new.ts"));
  if (!found.length) return undefined;
  file.write(root.root().commitEdits(found.map((node: any) => node.replace(`let ${node.getMatch("A").text()} = ${node.getMatch("B").text()};`))) + suffix);
  return { path: file.path, count: found.length };
}
"#;

#[test]
fn an_imported_callback_edits_renames_and_returns_outputs() {
    let h = Harness::new();
    h.write("src/a.ts", "var a = 1;\n");
    h.write("src/old.ts", "const b = 2;\n");
    h.bundle_file("scripts/migrate.ts", MIGRATE);
    h.bundle_file(
        "scripts/helpers.ts",
        "export const suffix = \"// migrated\\n\";\n",
    );
    let workflow = r#"
import { step, forEachFile } from "codemod/workflow";
import migrate from "./scripts/migrate.ts";
export default async function main() {
  const result = await step("Migrate", forEachFile({ include: "src/**/*.ts", language: "typescript" }, migrate));
  return { outputs: result.outputs, changes: result.changes };
}
"#;
    let report = h.start(workflow, Value::Null);
    let result = output(&report).unwrap();
    assert_eq!(
        result["outputs"],
        json!([{ "path": "src/a.ts", "count": 1 }])
    );
    assert_eq!(h.read("src/a.ts"), "let a = 1;\n// migrated\n");
    assert!(h.exists("src/new.ts") && !h.exists("src/old.ts"));
}

/// An imported callback's step is identified by the file and the files it
/// imports: changing either runs it again on resume.
#[test]
fn a_changed_imported_callback_runs_its_step_again_on_resume() {
    let h = Harness::new();
    h.write("a.txt", "a\n");
    h.bundle_file("scripts/helpers.ts", "export const mark = \"1\";\n");
    h.bundle_file(
        "scripts/tag.ts",
        "import { mark } from \"./helpers.ts\";\nexport default (file: any) => { file.write(file.text.trim() + mark + \"\\n\"); };\n",
    );
    let workflow = r#"
import { step, forEachFile } from "codemod/workflow";
import tag from "./scripts/tag.ts";
export default async function main() {
  await step("Tag", forEachFile("a.txt", tag));
  await step("Check", () => { throw new Error("not yet"); });
}
"#;
    let report = h.start(workflow, Value::Null);
    assert!(output(&report).is_err());
    assert_eq!(h.read("a.txt"), "a1\n");

    // Nothing changed: replayed, not run again.
    let resumed = h.resume(&report, None);
    assert!(output(&resumed).is_err());
    assert_eq!(h.read("a.txt"), "a1\n");

    // A file the callback imports changed: it runs again.
    h.bundle_file("scripts/helpers.ts", "export const mark = \"2\";\n");
    let resumed = h.resume(&report, None);
    assert!(output(&resumed).is_err());
    assert_eq!(h.read("a.txt"), "a12\n");
}

#[test]
fn shard_runs_its_filter_and_function_inline_or_imported() {
    let h = Harness::new();
    for name in ["a", "b", "c"] {
        h.write(&format!("src/{name}.ts"), "var x = 1;\n");
    }
    h.write("src/clean.ts", "let y = 2;\n");
    h.bundle_file(
        "scripts/shard.ts",
        r#"
export default function shardFiles(input: { files: string[] }) {
  return input.files.map((file, index) => ({ name: `one-${index}`, _meta_shard: index, _meta_files: [file] }));
}
"#,
    );
    let report = h.start(
        r#"
import { step, shard } from "codemod/workflow";
import shardFiles from "./scripts/shard.ts";
export default async function main() {
  const keyword = "var";
  const imported = await step("Imported", shard({
    method: { function: shardFiles },
    target: "src",
    files: "**/*.ts",
    filter: (file) => file.text.includes(keyword),
  }));
  const inline = await step("Inline", shard({
    method: { function: ({ files }) => [{ name: "all", _meta_shard: 0, _meta_files: files }] },
    target: "src",
    files: { include: "**/*.ts", exclude: "clean.ts" },
  }));
  return { imported: imported.map((s: any) => s._meta_files[0]).sort(), inline: inline[0]._meta_files.sort() };
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({
            "imported": ["src/a.ts", "src/b.ts", "src/c.ts"],
            "inline": ["src/a.ts", "src/b.ts", "src/c.ts"],
        })
    );
}

/// Only jssg transforms from workflow.yaml (the translation's adapter) may
/// change other files from a forEachFile callback; a callback of its own
/// still may not, and what it tried is not written.
#[test]
fn a_for_each_file_callback_cannot_change_other_files_with_jssg_transform() {
    let h = Harness::new();
    h.write("a.ts", "const a = 1;\n");
    h.write("other.ts", "const other = 1;\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { jssgTransform } from "codemod/ast-grep";
export default async function main({ target }) {
  await step("Edit", forEachFile("a.ts", async (file) => {
    await jssgTransform((root) => root.root().text().replace("1", "2"), `${target}/other.ts`, "typescript");
    file.write("const a = 2;\n");
  }));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("jssgTransform() is not supported in forEachFile callbacks"),
        "{error}"
    );
    assert_eq!(h.read("a.ts"), "const a = 1;\n");
    assert_eq!(h.read("other.ts"), "const other = 1;\n");
}

#[test]
fn a_function_step_renames_what_it_changes_with_jssg_transform() {
    let h = Harness::new();
    h.write("styles.less", "@color: red;\n");
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { jssgTransform } from "codemod/ast-grep";
export default async function main({ target }) {
  await step("Rename", async () => {
    await jssgTransform((root) => {
      root.rename("styles.css");
      return root.root().text().replace("@color", "--color");
    }, `${target}/styles.less`, "css");
  });
}
"#,
        Value::Null,
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert!(!h.exists("styles.less"));
    assert_eq!(h.read("styles.css"), "--color: red;\n");
}

/// The YAML translation's adapter can only take the file its callback runs
/// on: a callback that asks for another, in the target or outside it, is
/// refused, and nothing is read or written through it.
#[test]
fn a_callback_cannot_take_another_file_through_the_jssg_adapter_host() {
    let h = Harness::new();
    h.write("a.ts", "const a = 1;\n");
    h.write("other.ts", "const other = 1;\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  return await step("Edit", forEachFile("a.ts", (file) => {
    const errors = [];
    for (const path of ["other.ts", "../outside.ts"]) {
      try {
        globalThis.__jssgCompatHost.begin(path, false);
      } catch (error) {
        errors.push(String(error.message));
      }
    }
    file.write("const a = 2;\n");
    return errors;
  }));
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    assert_eq!(
        result["outputs"],
        json!([[
            "'other.ts' is not the file this callback runs on",
            "'../outside.ts' is not the file this callback runs on"
        ]]),
        "{result}"
    );
    assert_eq!(h.read("a.ts"), "const a = 2;\n");
    assert_eq!(h.read("other.ts"), "const other = 1;\n");
}

/// With several files in flight per worker, `jssgTransform` on other files
/// is refused, and what it did is not written, even when other files of
/// the step succeed.
#[test]
fn refused_jssg_transform_changes_are_not_written_with_concurrency() {
    let h = Harness::new();
    h.write("a.ts", "const a = 1;\n");
    h.write("b.ts", "const b = 1;\n");
    h.write("c.ts", "const c = 1;\n");
    h.write("other.txt", "other\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { jssgTransform } from "codemod/ast-grep";
export default async function main({ target }) {
  return await step("Edit", forEachFile({ include: "*.ts", concurrency: 2 }, async (file) => {
    if (file.path === "a.ts") {
      await jssgTransform((root) => root.root().text() + "changed\n", `${target}/other.txt`, "typescript");
    }
    file.write(file.text.replace("1", "2"));
  }));
}
"#,
        Value::Null,
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("other.txt"), "other\n");
    assert_eq!(h.read("c.ts"), "const c = 2;\n");
    assert_eq!(h.read("a.ts"), "const a = 1;\n");
}

/// Work a callback leaves running (a timer it does not wait for, an
/// interval it never clears) does not crash the run when the step's
/// workers finish.
#[test]
fn work_a_callback_leaves_running_does_not_crash_the_run() {
    let h = Harness::new();
    h.write("a.ts", "const a = 1;\n");
    h.write("b.ts", "const b = 1;\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("Edit", forEachFile("*.ts", (file) => {
    if (file.path === "a.ts") new Promise((resolve) => setTimeout(resolve, 50)).then(() => {});
    else setInterval(() => {}, 10);
    file.write(file.text.replace("1", "2"));
  }));
}
"#,
        Value::Null,
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(h.read("a.ts"), "const a = 2;\n");
    assert_eq!(h.read("b.ts"), "const b = 2;\n");
}

/// A function step that returns with a timer or an interval still pending
/// does not crash the run either.
#[test]
fn work_a_function_step_leaves_running_does_not_crash_the_run() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
export default async function main() {
  const a = await step("Timer", () => { setTimeout(() => {}, 10_000); return 1; });
  const b = await step("Interval", () => { setInterval(() => {}, 10); return 2; });
  return [a, b];
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([1, 2]));
}

/// Nor does a step that reads the dependency graph and leaves an interval running.
#[test]
fn work_a_dependency_graph_callback_leaves_running_does_not_crash_the_run() {
    let h = Harness::new();
    yarn_target(&h);
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { dependencies } from "codemod/deps";
export default async function main() {
  return await step(() => ((g) => { setInterval(() => {}, 10); setTimeout(() => {}, 10_000); return g.node("dnd").name; })(dependencies()));
}
"#,
        Value::Null,
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
}

/// What every step of a dry run returns, and what it reports it would
/// change in all, as a real run of the same workflow does it.
const LATER_STEPS_SEE_EARLIER_ONES: &str = r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
import { jssgTransform } from "codemod/ast-grep";
export default async function main({ target }) {
  await step("One", forEachFile("**/*.ts", (file) => {
    if (file.path === "gone.ts") return file.delete();
    file.write(file.text.replace("1", "2"));
    if (file.path === "old.ts") file.rename("new.ts");
  }));
  const seen = await step("Two", forEachFile("**/*.ts", (file) => {
    file.write(file.text.replace("2", "3"));
    return file.path;
  }));
  const rules = await step("Three", forEachFile(astGrep({ id: "let", language: "typescript", rule: { pattern: "const $A = 3" }, fix: "let $A = 3;" })));
  await step("Four", async () => {
    await jssgTransform((root) => root.root().text().replace("let", "var"), `${target}/a.ts`, "typescript");
  });
  return { seen: seen.outputs, matches: rules.matches };
}
"#;

fn later_steps_target(h: &Harness) {
    h.write("a.ts", "const a = 1;\n");
    h.write("old.ts", "const o = 1;\n");
    h.write("gone.ts", "const g = 1;\n");
}

/// In a dry run each step reads what the earlier ones would have left:
/// edited text, renamed files at their new paths, deleted files gone. The
/// diff it reports at the end is a real run's, and nothing is written.
#[test]
fn a_dry_run_lets_later_steps_see_what_earlier_ones_would_change() {
    use butterflow_core::diff::{generate_unified_diff, DiffConfig, DiffMetadata};
    let real = Harness::new();
    later_steps_target(&real);
    let real_report = real.start(LATER_STEPS_SEE_EARLIER_ONES, Value::Null);
    let expected_output = json!({ "seen": ["a.ts", "new.ts"], "matches": 2 });
    assert_eq!(output(&real_report).unwrap(), expected_output);
    assert_eq!(real.read("a.ts"), "var a = 3;\n");
    assert_eq!(real.read("new.ts"), "let o = 3;\n");
    assert!(!real.exists("old.ts") && !real.exists("gone.ts"));

    let dry = Harness::new();
    later_steps_target(&dry);
    let dry_report = dry.dry_run(LATER_STEPS_SEE_EARLIER_ONES);
    assert_eq!(output(&dry_report).unwrap(), expected_output);
    assert_eq!(dry.read("a.ts"), "const a = 1;\n");
    assert_eq!(dry.read("old.ts"), "const o = 1;\n");
    assert_eq!(dry.read("gone.ts"), "const g = 1;\n");
    assert!(!dry.exists("new.ts"));

    let diffs: Vec<(String, String)> = dry
        .events
        .take()
        .into_iter()
        .filter_map(|event| match event {
            Event::RunDiff { path, diff, .. } => Some((path, diff)),
            _ => None,
        })
        .collect();
    let diff = |path: &str, before: &str, after: &str| {
        let config = DiffConfig {
            color: false,
            ..DiffConfig::default()
        };
        let diff = generate_unified_diff(
            std::path::Path::new(path),
            before,
            after,
            &config,
            DiffMetadata::default(),
        );
        (path.to_string(), diff.diff_text)
    };
    assert_eq!(
        diffs,
        vec![
            diff("a.ts", "const a = 1;\n", &real.read("a.ts")),
            diff("gone.ts", "const g = 1;\n", ""),
            diff("new.ts", "", &real.read("new.ts")),
            diff("old.ts", "const o = 1;\n", ""),
        ]
    );
    assert!(diffs[0].1.contains("+var a = 3;"), "{diffs:?}");
}

/// `shard` reads files through a dry run's overlay too: it shards the files
/// earlier steps would have renamed at their new paths, not the deleted
/// ones, and its filter sees their new text.
#[test]
fn a_dry_run_shards_what_earlier_steps_would_have_left() {
    let source = r#"
import { step, forEachFile, shard } from "codemod/workflow";
export default async function main() {
  await step("Move", forEachFile("src/**/*.ts", (file) => {
    if (file.path === "src/one/gone.ts") return file.delete();
    if (file.path === "src/one/b.ts") file.write("export const keep = 1;\n");
    if (file.path === "src/old/a.ts") file.rename("src/new/a.ts");
  }));
  const all = await step("All", shard({ method: { type: "directory", maxFilesPerShard: 10 }, target: "src", files: "**/*.ts" }));
  const kept = await step("Kept", shard({ method: { type: "directory", maxFilesPerShard: 10 }, target: "src", files: "**/*.ts", filter: (file) => file.text.includes("keep") }));
  return { all, kept };
}
"#;
    let target = |h: &Harness| {
        h.write("src/one/b.ts", "export const b = 1;\n");
        h.write("src/one/gone.ts", "export const g = 1;\n");
        h.write("src/old/a.ts", "export const keep = 1;\n");
        h.write("src/two/c.ts", "export const c = 1;\n");
    };
    let real = Harness::new();
    target(&real);
    let real = output(&real.start(source, Value::Null)).unwrap();
    let dry = Harness::new();
    target(&dry);
    let dry = output(&dry.dry_run(source)).unwrap();
    let files = |result: &Value| {
        let mut files: Vec<String> = shard_files(result)
            .into_iter()
            .flat_map(|(_, files)| files)
            .collect();
        files.sort();
        files
    };
    assert_eq!(
        files(&real["all"]),
        ["src/new/a.ts", "src/one/b.ts", "src/two/c.ts"]
    );
    assert_eq!(files(&real["kept"]), ["src/new/a.ts", "src/one/b.ts"]);
    assert_eq!(shard_files(&dry["all"]), shard_files(&real["all"]));
    assert_eq!(shard_files(&dry["kept"]), shard_files(&real["kept"]));
}

/// A real run reports no run diff: its steps' diffs are in the summary.
#[test]
fn only_a_dry_run_reports_its_diff_when_it_ends() {
    let h = Harness::new();
    h.write("a.txt", "1\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() { await step(forEachFile("a.txt", (file) => file.write("2\n"))); }
"#,
        Value::Null,
    );
    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert!(!h
        .events
        .take()
        .iter()
        .any(|event| matches!(event, Event::RunDiff { .. })));
}

mod common;

use std::sync::Arc;
use std::time::Duration;

use codemod_workflow::events::{Event, StepAt};
use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::{
    build, cancel_run, Canceled, Ending, EngineError, LocalStore, MemoryStore, RunStatus, RunStore,
    StepStatus,
};
use common::{output, Harness};
use serde_json::{json, Value};

fn build_error(source: &str) -> String {
    build("workflow.ts", source)
        .expect_err("expected a build error")
        .to_string()
}

// ---------------------------------------------------------------- build ----

#[test]
fn assigning_to_an_outside_variable_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  let count = 0;
  await step(forEachFile("**/*.ts", (file) => { count++; }));
}
"#,
    );
    assert!(error.contains("assigns to 'count'"), "{error}");
    assert!(error.starts_with("workflow.ts:5:"), "{error}");
}

#[test]
fn capturing_a_reassigned_local_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  let prefix = "a";
  prefix = "b";
  await step(forEachFile("**/*.ts", (file) => file.path.startsWith(prefix)));
}
"#,
    );
    assert!(
        error.contains("captures 'prefix', which is reassigned"),
        "{error}"
    );
}

#[test]
fn capturing_a_local_function_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const shout = (s: string) => s.toUpperCase();
  await step(forEachFile("**/*.ts", (file) => { file.write(shout(file.text)); }));
}
"#,
    );
    assert!(error.contains("captures 'shout'"), "{error}");
}

#[test]
fn using_a_reassigned_module_binding_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod/workflow";
let suffix = ".js";
suffix = ".mjs";
export default async function main() {
  await step(forEachFile("**/*.ts", (file) => { file.rename(file.path + suffix); }));
}
"#,
    );
    assert!(
        error.contains("'suffix', a module-level binding that is reassigned"),
        "{error}"
    );
}

#[test]
fn starting_a_step_inside_a_callback_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step(forEachFile("**/*.ts", (file) => { step(() => 1); }));
}
"#,
    );
    assert!(
        error.contains("uses 'step' inside a forEachFile callback"),
        "{error}"
    );
}

#[test]
fn a_callback_passed_by_name_is_a_build_error() {
    let error = build_error(
        r#"
import { step, forEachFile } from "codemod/workflow";
const touch = (file: any) => file.write(file.text);
export default async function main() {
  await step(forEachFile("**/*.ts", touch));
}
"#,
    );
    assert!(error.contains("must be written inline"), "{error}");
}

#[test]
fn module_level_helpers_are_copied_into_the_callback_with_their_dependencies() {
    let built = build(
        "workflow.ts",
        r#"
import { step, forEachFile } from "codemod/workflow";
const MARK = "// migrated";
const unused = 42;
function mark(text: string): string { return `${MARK}\n${text}`; }
export default async function main() {
  const extra = "!";
  await step(forEachFile("**/*.ts", (file) => { file.write(mark(file.text) + extra); }));
}
"#,
    )
    .unwrap();
    assert_eq!(built.callbacks.len(), 1);
    let callback = built.callbacks.values().next().unwrap();
    assert!(callback.contains("const MARK"), "{callback}");
    assert!(callback.contains("function mark"), "{callback}");
    assert!(!callback.contains("unused"), "{callback}");
    assert!(callback.contains("extra"), "{callback}");
    assert!(built.main.contains("() => ({ extra })"), "{}", built.main);
}

// ------------------------------------------------------------------ run ----

#[test]
fn renames_files_without_parsing_them() {
    let h = Harness::new();
    h.write("src/a.less", ".a {}");
    h.write("src/nested/b.less", ".b {}");
    h.write("src/c.ts", "export {}");

    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const moved = await step("Rename LESS files", forEachFile("src/**/*.less", (file) => {
    file.rename(file.path.replace(/\.less$/, ".css"));
  }));
  return moved.changes;
}
"#,
        Value::Null,
    );

    let changes = output(&report).unwrap();
    assert_eq!(
        changes["renamed"],
        json!([
            { "from": "src/a.less", "to": "src/a.css" },
            { "from": "src/nested/b.less", "to": "src/nested/b.css" },
        ])
    );
    assert_eq!(h.read("src/a.css"), ".a {}");
    assert!(!h.exists("src/a.less"));
    assert_eq!(report.steps[0].label, "Rename LESS files");
    assert_eq!(report.run.status, RunStatus::Succeeded);
}

const MIGRATE_MOMENT: &str = r#"
import { step, forEachFile } from "codemod/workflow";
import { parse } from "codemod/ast-grep";

const LEGACY = "moment";

function importsOf(root: any, name: string): any[] {
  return root.root().findAll({ rule: { pattern: `import $X from "${name}"` } });
}

export default async function main() {
  const usages = await step("Find moment imports",
    forEachFile({ language: "typescript", containing: LEGACY }, (file) => {
      const found = importsOf(parse(file), LEGACY);
      return found.length ? { path: file.path, count: found.length } : undefined;
    }),
  );

  const paths = usages.outputs.map((usage: { path: string }) => usage.path);

  await step(forEachFile({ language: "typescript" }, (file) => {
    if (!paths.includes(file.path)) return;
    const root = parse(file);
    const edits = importsOf(root, LEGACY).map((node: any) =>
      node.replace(`import ${node.getMatch("X").text()} from "dayjs";`),
    );
    file.write(root.root().commitEdits(edits));
  }));

  return { migrated: paths };
}
"#;

#[test]
fn a_later_step_captures_an_earlier_step_result_and_parses_files() {
    let h = Harness::new();
    h.write("src/a.ts", "import moment from \"moment\";\nmoment();\n");
    h.write("src/b.ts", "import other from \"other\";\n");
    h.write("src/c.ts", "// mentions moment only in a comment\n");

    let report = h.start(MIGRATE_MOMENT, Value::Null);

    assert_eq!(
        output(&report).unwrap(),
        json!({ "migrated": ["src/a.ts"] })
    );
    assert_eq!(
        h.read("src/a.ts"),
        "import moment from \"dayjs\";\nmoment();\n"
    );
    assert_eq!(h.read("src/b.ts"), "import other from \"other\";\n");
    assert!(report.steps[1].key.contains("src/a.ts"));
}

#[test]
fn a_dry_run_reports_changes_without_writing_or_persisting() {
    let h = Harness::new();
    h.write("src/a.ts", "import moment from \"moment\";\n");

    let report = h.dry_run(MIGRATE_MOMENT);

    assert!(output(&report).is_ok(), "{:?}", report.ending);
    assert_eq!(
        report.steps[1].result.as_ref().unwrap()["changes"]["written"],
        json!(["src/a.ts"])
    );
    assert_eq!(h.read("src/a.ts"), "import moment from \"moment\";\n");
    assert!(h.store.list_runs(10).unwrap().is_empty());
}

const TOUCH_THEN_CHECK: &str = r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step("Touch", forEachFile("*.txt", (file) => { file.write(file.text + "+"); }));
  await step("Check", () => "ok");
}
"#;

fn broken_check() -> String {
    TOUCH_THEN_CHECK.replace("() => \"ok\"", "() => { throw new Error(\"not yet\"); }")
}

#[test]
fn fixing_the_failed_step_and_resuming_replays_the_finished_ones() {
    let h = Harness::new();
    h.write("a.txt", "a");

    let first = h.start(&broken_check(), Value::Null);
    let error = output(&first).unwrap_err();
    assert!(error.contains("step 'Check' failed: not yet"), "{error}");
    assert_eq!(first.run.status, RunStatus::Failed);
    assert_eq!(first.steps[1].status, StepStatus::Failed);

    let second = h.resume(&first, Some(TOUCH_THEN_CHECK));
    assert!(output(&second).is_ok(), "{:?}", second.ending);
    assert_eq!(second.run.id, first.run.id);
    assert_eq!(second.run.status, RunStatus::Succeeded);
    // "Touch" was replayed, not run again.
    assert_eq!(h.read("a.txt"), "a+");
    assert_eq!(second.steps[1].status, StepStatus::Succeeded);
    assert!(h.events.take().iter().any(|e| matches!(
        e,
        Event::StepReplayed { at, label, .. } if at.position == 0 && label == "Touch"
    )));
}

#[test]
fn a_finished_step_that_changed_runs_again_on_resume() {
    let h = Harness::new();
    h.write("a.txt", "a");

    let first = h.start(&broken_check(), Value::Null);
    let changed = TOUCH_THEN_CHECK.replace("+ \"+\"", "+ \"!\"");
    let second = h.resume(&first, Some(&changed));

    assert!(output(&second).is_ok(), "{:?}", second.ending);
    assert_eq!(h.read("a.txt"), "a+!");
    // The failed step kept its place; the changed one was added.
    let steps: Vec<_> = second
        .steps
        .iter()
        .map(|s| (s.position, s.label.as_str(), s.status))
        .collect();
    assert_eq!(
        steps,
        [
            (0, "Touch", StepStatus::Succeeded),
            (1, "Check", StepStatus::Succeeded),
            (2, "Touch", StepStatus::Succeeded),
        ]
    );
}

#[test]
fn a_step_added_before_the_failed_one_does_not_disturb_the_finished_ones() {
    let h = Harness::new();
    h.write("a.txt", "a");

    let first = h.start(&broken_check(), Value::Null);
    let with_setup = TOUCH_THEN_CHECK.replace(
        "  await step(\"Touch\"",
        "  await step(\"Setup\", () => \"ready\");\n  await step(\"Touch\"",
    );
    let second = h.resume(&first, Some(&with_setup));

    assert!(output(&second).is_ok(), "{:?}", second.ending);
    assert_eq!(h.read("a.txt"), "a+", "Touch did not run again");
    let events = h.events.take();
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepStarted { at, label, .. } if at.position == 2 && label == "Setup"
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepReplayed { at, label, .. } if at.position == 0 && label == "Touch"
    )));
}

#[test]
fn a_succeeded_run_cannot_be_resumed() {
    let h = Harness::new();
    let report = h.start(TOUCH_THEN_CHECK, Value::Null);
    assert!(h.engine.resume(&report.run.id).is_err());
}

#[test]
fn steps_are_persisted_as_they_finish() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Arc::new(LocalStore::new(dir.path()));
    let h = Harness::with(
        store.clone(),
        Arc::new(NonInteractive { approve_all: false }),
    );
    h.write("a.txt", "a");

    let report = h.start(&broken_check(), Value::Null);

    let saved = store.load_steps(&report.run.id).unwrap();
    assert_eq!(saved, report.steps);
    assert_eq!(saved[0].status, StepStatus::Succeeded);
    assert_eq!(saved[1].status, StepStatus::Failed);
    let run = store.get_run(&report.run.id).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Failed);
    assert!(run.error.unwrap().contains("not yet"));
}

#[test]
fn params_reach_the_workflow() {
    let h = Harness::new();
    let report = h.start(
        r#"
export default async function main({ params }: { params: { name: string } }) {
  return `hello ${params.name}`;
}
"#,
        json!({ "name": "codemod" }),
    );
    assert_eq!(output(&report).unwrap(), json!("hello codemod"));
}

#[test]
fn a_step_given_a_state_option_fails() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
export default async function main() { await step("Old", () => 1, { state: { x: 1 } } as any); }
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("steps take no `state`"), "{error}");
}

#[test]
fn events_report_each_step() {
    let h = Harness::new();
    h.write("a.txt", "a");
    h.start(TOUCH_THEN_CHECK, Value::Null);

    let events = h.events.take();
    assert!(matches!(
        events.first(),
        Some(Event::RunStarted { resumed: false, .. })
    ));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepStarted { at, label, kind } if at == &StepAt::top(0) && label == "Touch" && kind.is_some()
    )));
    assert!(events.contains(&Event::StepProgress {
        at: StepAt::top(0),
        processed: 1,
        total: 1
    }));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepFinished {
            at,
            status: StepStatus::Succeeded,
            ..
        } if at.position == 1
    )));
    assert!(matches!(
        events.last(),
        Some(Event::RunFinished {
            status: RunStatus::Succeeded,
            ..
        })
    ));
}

#[test]
fn the_stop_handle_stops_a_running_workflow() {
    let h = Harness::new();
    let stop = h.engine.stop_handle();
    let workflow = h.workflow(
        r#"
import { step } from "codemod/workflow";
export default async function main() {
  await step("Spin", () => { while (true) {} });
}
"#,
    );
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        stop.stop();
    });
    let report = h.start_loaded(workflow, Value::Null, false);
    // Ctrl-C pauses: the run can be resumed.
    assert_eq!(report.ending, Ending::Paused);
    assert_eq!(report.run.status, RunStatus::Paused);
    assert_eq!(
        report.run.status.resumable(),
        codemod_workflow::Resumable::Yes
    );
}

#[test]
fn stopping_to_cancel_ends_the_run_for_good() {
    let h = Harness::new();
    let stop = h.engine.stop_handle();
    let workflow = h.workflow(
        r#"
import { step } from "codemod/workflow";
export default async function main() {
  await step("Spin", () => { while (true) {} });
}
"#,
    );
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        stop.stop();
        stop.stop_with(codemod_workflow::StopMode::Cancel);
    });
    let report = h.start_loaded(workflow, Value::Null, false);
    assert_eq!(report.ending, Ending::Canceled);
    assert_eq!(report.run.status, RunStatus::Canceled);
    let error = h.engine.resume(&report.run.id).unwrap_err().to_string();
    assert!(error.contains("--force"), "{error}");
}

#[test]
fn a_capture_that_is_not_json_fails_when_the_step_starts() {
    let h = Harness::new();
    h.write("a.ts", "");

    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const seen = new Set(["a.ts"]);
  await step(forEachFile("*.ts", (file) => seen.has(file.path)));
}
"#,
        Value::Null,
    );

    let error = output(&report).unwrap_err();
    assert!(
        error.contains("captures.seen is a Set, which is not JSON"),
        "{error}"
    );
    assert!(report.steps.is_empty());
}

#[test]
fn mutating_a_capture_inside_a_callback_fails_that_file() {
    let h = Harness::new();
    h.write("a.ts", "");

    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const seen: string[] = [];
  return step(forEachFile("*.ts", (file) => { seen.push(file.path); }));
}
"#,
        Value::Null,
    );

    // The only file failed, so the step did.
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("the callback failed on every file; first error: a.ts: TypeError"),
        "{error}"
    );
    assert!(error.contains("are frozen"), "{error}");
}

#[test]
fn a_step_result_is_frozen_in_the_workflow() {
    let h = Harness::new();
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
export default async function main() {
  const result = await step(() => ({ items: [1, 2] }));
  try { result.items.push(3); } catch (e) { return "frozen"; }
  return "mutable";
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!("frozen"));
}

#[test]
fn steps_started_together_both_run() {
    let h = Harness::new();
    h.write("a.txt", "a");
    h.write("b.md", "b");

    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const [txt, md] = await Promise.all([
    step(forEachFile("*.txt", (file) => file.path)),
    step(forEachFile("*.md", (file) => file.path)),
  ]);
  return [...txt.outputs, ...md.outputs];
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["a.txt", "b.md"]));
    assert_eq!(report.steps.len(), 2);
}

#[test]
fn conflicting_renames_fail_the_step_and_write_nothing() {
    let h = Harness::new();
    h.write("a.txt", "a");
    h.write("b.txt", "b");

    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step(forEachFile("*.txt", (file) => { file.rename("same.txt"); }));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("are both renamed to 'same.txt'"), "{error}");
    assert!(h.exists("a.txt"));
    assert!(!h.exists("same.txt"));
}

#[test]
fn the_older_codemod_colon_spelling_still_imports_the_same_modules() {
    let h = Harness::new();
    h.write("a.ts", "const a = 1;\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod:workflow";
import { step as slashStep } from "codemod/workflow";
import { parse } from "codemod:ast-grep";
export default async function main() {
  if (step !== slashStep) throw new Error("two workflow modules");
  const found = await step(forEachFile("**/*.ts", (file) => {
    return parse("typescript", file.text).root().findAll("const $A = $B").length;
  }));
  return found.outputs;
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([1]));
}

#[test]
fn path_named_exports_work_in_the_quickjs_body() {
    let h = Harness::new();
    h.bundle_file("helper.ts", "export { sep, delimiter } from 'path';");
    let report = h.start(
        r#"
import path, { sep, delimiter, join } from "node:path";
import * as barePath from "path";
import * as helper from "./helper.ts";
export default function main() {
  return {
    sep, delimiter,
    joined: join("one", "two"),
    sameDefault: path === barePath.default,
    sameNamed: sep === path.sep && delimiter === path.delimiter && join === path.join,
    sameReexports: sep === helper.sep && delimiter === helper.delimiter,
  };
}
"#,
        Value::Null,
    );
    let sep = std::path::MAIN_SEPARATOR.to_string();
    assert_eq!(
        output(&report).unwrap(),
        json!({
            "sep": sep,
            "delimiter": if cfg!(windows) { ";" } else { ":" },
            "joined": format!("one{sep}two"),
            "sameDefault": true,
            "sameNamed": true,
            "sameReexports": true,
        })
    );
}

#[test]
fn importing_a_node_only_codemod_export_is_a_build_error() {
    let h = Harness::new();
    let error = codemod_workflow::build(
        "workflow.ts",
        r#"
import { runWorkflow } from "codemod/testing";
export default async function main() { return runWorkflow; }
"#,
    )
    .err()
    .map(|error| error.to_string())
    .unwrap_or_default();
    drop(h);
    assert!(
        error.contains("'codemod/testing' is not a module workflows can import"),
        "{error}"
    );
}

#[test]
fn language_maps_import_under_either_spelling() {
    // They are types only, so the import is gone once types are stripped.
    for specifier in [
        "codemod/ast-grep/langs/tsx",
        "codemod:ast-grep/langs/tsx",
        "codemod/langs/tsx",
    ] {
        let source = format!(
            "import TSX from \"{specifier}\";\nexport default async function main() {{ const x: TSX | null = null; return x; }}\n"
        );
        assert!(
            codemod_workflow::build("workflow.ts", &source).is_ok(),
            "{specifier}"
        );
    }
}

#[test]
fn a_callback_in_an_imported_file_is_a_build_error() {
    let h = Harness::new();
    h.bundle_file(
        "steps.ts",
        r#"
import { step, forEachFile } from "codemod/workflow";
export const rewrite = () => step(forEachFile("**/*.ts", (file) => { file.write(""); }));
"#,
    );
    let path = h.bundle.path().join("workflow.ts");
    std::fs::write(
        &path,
        "import { rewrite } from \"./steps.ts\";\nexport default async function main() { await rewrite(); }\n",
    )
    .unwrap();
    let error = codemod_workflow::load_workflow(&path)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("steps.ts calls forEachFile() with a callback"),
        "{error}"
    );

    // Starting host tasks from a helper is fine: nothing to extract.
    h.bundle_file(
        "steps.ts",
        "import { step, shell } from \"codemod/workflow\";\nexport const hello = () => step(shell(\"echo hello\"));\n",
    );
    std::fs::write(
        &path,
        "import { hello } from \"./steps.ts\";\nexport default async function main() { await hello(); }\n",
    )
    .unwrap();
    assert!(codemod_workflow::load_workflow(&path).is_ok());
}

#[test]
fn a_workflow_loaded_by_a_relative_path_resolves_imports_from_its_own_directory() {
    let bundle = tempfile::tempdir_in(".").unwrap();
    std::fs::create_dir(bundle.path().join("src")).unwrap();
    std::fs::write(
        bundle.path().join("src/helper.ts"),
        "export const answer = 42;\n",
    )
    .unwrap();
    std::fs::write(
        bundle.path().join("src/workflow.ts"),
        "import { answer } from \"./helper.ts\";\nexport default async function main() { return answer; }\n",
    )
    .unwrap();

    // Loaded the way the CLI is given it: relative to the working directory, e.g. `.tmpXXXX/src/workflow.ts`.
    let relative = std::path::Path::new(bundle.path().file_name().unwrap()).join("src/workflow.ts");

    assert!(codemod_workflow::load_workflow(&relative).is_ok());
}

#[test]
fn importing_the_workflow_module_as_a_namespace_is_a_build_error() {
    let error = build_error(
        r#"
import * as wf from "codemod/workflow";
export default async function main() {
  await wf.step(wf.forEachFile("**/*.ts", (file) => {}));
}
"#,
    );
    assert!(error.contains("by name"), "{error}");
}

#[test]
fn importing_a_dependency_api_from_the_workflow_module_says_it_moved_to_codemod_deps() {
    let error = build_error(
        r#"
import { step, semver } from "codemod/workflow";
export default async function main() {
  await step(() => semver.compare("1.0.0", "2.0.0"));
}
"#,
    );
    assert!(
        error.contains(r#"import { semver } from "codemod/deps""#),
        "{error}"
    );
    // dependencyGraph is gone from both modules.
    let error = build_error(
        r#"
import { step, dependencyGraph } from "codemod/workflow";
export default async function main() {
  await step(dependencyGraph({}, (g) => g.workspaces()));
}
"#,
    );
    assert!(error.contains("dependencyGraph() was removed"), "{error}");
}

#[test]
fn a_callback_keeps_its_id_through_comments_and_either_module_spelling() {
    let ids = |source: &str| -> Vec<String> {
        build("workflow.ts", source)
            .unwrap()
            .callbacks
            .keys()
            .cloned()
            .collect()
    };
    let plain = ids(r#"
import { step } from "codemod/workflow";
import { parse } from "codemod/ast-grep";
export default async function main() {
  await step("Count", () => parse("typescript", "const a = 1").root().text().length);
}
"#);
    let commented = ids(r#"
import { step } from "codemod:workflow";
import { parse } from "codemod:ast-grep";
export default async function main() {
  await step("Count", () => {
    // how long is it?
    return parse("typescript", "const a = 1").root().text().length;
  });
}
"#);
    let changed = ids(r#"
import { step } from "codemod/workflow";
import { parse } from "codemod/ast-grep";
export default async function main() {
  await step("Count", () => parse("typescript", "const a = 2").root().text().length);
}
"#);
    assert_eq!(plain.len(), 1);
    assert_ne!(plain, changed, "a change in the code is a new id");
    // The spelling of the modules and a comment are not changes. (Braces
    // around the body are, so compare the spelling alone.)
    let other_spelling = ids(r#"
import { step } from "codemod:workflow";
import { parse } from "codemod:ast-grep";
export default async function main() {
  await step("Count", () => parse("typescript", "const a = 1").root().text().length);
}
"#);
    assert_eq!(plain, other_spelling);
    let with_comment = ids(r#"
import { step } from "codemod:workflow";
import { parse } from "codemod:ast-grep";
export default async function main() {
  await step("Count", () => {
    // how long is it, again?
    return parse("typescript", "const a = 1").root().text().length;
  });
}
"#);
    assert_eq!(commented, with_comment);
}

#[test]
fn imports_keep_the_order_they_were_written_in() {
    let built = build(
        "workflow.ts",
        r#"
import "./polyfill.ts";
import { zeta } from "./zeta.ts";
import { step } from "codemod/workflow";
import { alpha } from "./alpha.ts";
export default async function main() {
  await step("Use", () => zeta + alpha);
  return zeta + alpha;
}
"#,
    )
    .unwrap();
    let order = |text: &str| -> Vec<usize> {
        ["polyfill", "zeta.ts", "alpha.ts"]
            .iter()
            .filter_map(|name| text.find(name))
            .collect()
    };
    let body = order(&built.main);
    assert_eq!(body.len(), 3, "{}", built.main);
    assert!(body.is_sorted(), "{}", built.main);
    let callback = built.callbacks.values().next().unwrap();
    let in_callback = order(callback);
    assert_eq!(in_callback.len(), 2, "{callback}");
    assert!(in_callback.is_sorted(), "{callback}");
}

// ------------------------------------------------------------ lifecycle ----

const TWO_STEPS: &str = r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  await step("One", shell("echo one"));
  await step("Two", shell(`test -f ${target}/go`));
  return "done";
}
"#;

fn approving_with(store: Arc<dyn RunStore>) -> Harness {
    Harness::with(store, Arc::new(NonInteractive { approve_all: true }))
}

#[test]
fn a_run_stopped_through_the_store_can_be_resumed() {
    let store: Arc<dyn RunStore> = Arc::new(MemoryStore::default());
    let h = approving_with(Arc::clone(&store));
    let first = h.start(TWO_STEPS, Value::Null);
    assert_eq!(first.run.status, RunStatus::Failed);

    // A cancel request left behind (the run ended before reading it) must
    // not cancel the next execution.
    store.request_cancel(&first.run.id).unwrap();
    h.write("go", "");
    let second = h.resume(&first, None);
    assert_eq!(output(&second).unwrap(), json!("done"));
    assert!(!store.cancel_requested(&first.run.id).unwrap());
}

#[test]
fn a_resume_that_cannot_load_the_workflow_leaves_the_run_as_it_was() {
    let store: Arc<dyn RunStore> = Arc::new(MemoryStore::default());
    let h = approving_with(Arc::clone(&store));
    let first = h.start(TWO_STEPS, Value::Null);
    let before = store.get_run(&first.run.id).unwrap().unwrap();

    std::fs::write(&first.run.workflow_path, "export default function main( {").unwrap();
    assert!(matches!(
        h.engine.resume(&first.run.id),
        Err(EngineError::Build(_))
    ));
    assert_eq!(store.get_run(&first.run.id).unwrap().unwrap(), before);
    assert!(!store.is_held(&first.run.id).unwrap());
}

#[test]
fn a_run_another_executor_holds_cannot_be_resumed_and_is_asked_to_stop() {
    let store: Arc<dyn RunStore> = Arc::new(MemoryStore::default());
    let h = approving_with(Arc::clone(&store));
    let first = h.start(TWO_STEPS, Value::Null);

    let lease = store.acquire(&first.run.id).unwrap().unwrap();
    assert!(matches!(
        h.engine.resume(&first.run.id),
        Err(EngineError::Busy(_))
    ));
    assert_eq!(
        cancel_run(store.as_ref(), &first.run.id).unwrap(),
        Canceled::Requested
    );
    drop(lease);
    assert!(h.engine.resume(&first.run.id).is_ok());
}

#[test]
fn a_run_whose_process_died_is_canceled_directly() {
    let store: Arc<dyn RunStore> = Arc::new(MemoryStore::default());
    let h = approving_with(Arc::clone(&store));
    let first = h.start(TWO_STEPS, Value::Null);
    // As a killed process leaves it: running on record, held by nobody.
    let mut record = store.get_run(&first.run.id).unwrap().unwrap();
    record.reopen(false).unwrap();
    store.put_run(&record).unwrap();

    assert_eq!(
        cancel_run(store.as_ref(), &first.run.id).unwrap(),
        Canceled::Marked
    );
    let record = store.get_run(&first.run.id).unwrap().unwrap();
    assert_eq!(record.status, RunStatus::Canceled);
    assert!(!store.cancel_requested(&first.run.id).unwrap());
}

#[test]
fn steps_still_running_when_another_fails_finish_and_are_recorded() {
    let h = approving_with(Arc::new(MemoryStore::default()));
    let report = h.start(
        r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  await Promise.all([
    step("Fails", shell("exit 1")),
    step("Slow", shell(`sleep 0.4 && echo slow > ${target}/slow.txt`)),
  ]);
}
"#,
        Value::Null,
    );
    assert!(output(&report).unwrap_err().contains("step 'Fails' failed"));
    assert!(h.exists("slow.txt"), "the run waited for the slow step");
    let slow = report.steps.iter().find(|s| s.label == "Slow").unwrap();
    assert_eq!(slow.status, StepStatus::Succeeded);
    // The run's last event comes after every step's.
    let events = h.events.take();
    assert!(matches!(events.last(), Some(Event::RunFinished { .. })));
}

#[test]
fn a_command_is_done_when_it_exits_whatever_else_is_running() {
    let h = approving_with(Arc::new(MemoryStore::default()));
    // "waits" can only succeed if "quick" is reported done while it runs.
    let report = h.start(
        r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  const quick = step("quick", shell("true")).then(() =>
    step("mark", shell(`touch ${target}/mark`)),
  );
  const waits = step(
    "waits",
    shell(`for i in $(seq 100); do test -f ${target}/mark && exit 0; sleep 0.05; done; exit 1`),
  );
  await Promise.all([quick, waits]);
  return "done";
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!("done"));
}

/// Two branches of two steps each. The order the second steps are issued in
/// follows the order the first steps finish in.
const TWO_BRANCHES: &str = r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  const a = async () => {
    await step("a1", shell("true"));
    await step("a2", shell(`echo a2 >> ${target}/log.txt`));
  };
  const b = async () => {
    await step("b1", shell(`for i in $(seq 100); do test -f ${target}/log.txt && exit 0; sleep 0.05; done; exit 1`));
    await step("b2", shell(`test -f ${target}/go && echo b2 >> ${target}/log.txt`));
  };
  await Promise.all([a(), b()]);
  return "done";
}
"#;

#[test]
fn branches_that_ran_side_by_side_resume_without_running_a_step_twice() {
    let h = approving_with(Arc::new(MemoryStore::default()));
    let first = h.start(TWO_BRANCHES, Value::Null);
    assert!(output(&first).unwrap_err().contains("step 'b2' failed"));
    assert_eq!(h.read("log.txt"), "a2\n");

    h.write("go", "");
    let second = h.resume(&first, None);
    assert_eq!(output(&second).unwrap(), json!("done"));
    assert_eq!(h.read("log.txt"), "a2\nb2\n", "a2 did not run again");
}

/// One branch handles a failed step while the other is still on its first
/// step. On resume the failed step runs again, live, and the other branch's
/// recorded step settles at once, so the steps after them are issued in the
/// other order than the first time.
const HANDLED_FAILURE_BESIDE_A_SLOW_BRANCH: &str = r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  const a = async () => {
    try {
      await step("a1", shell("exit 1"));
    } catch {
      await step("fallback", shell(`echo fallback >> ${target}/log.txt`));
    }
  };
  const b = async () => {
    await step("b1", shell(`for i in $(seq 100); do test -f ${target}/log.txt && exit 0; sleep 0.05; done; exit 1`));
    await step("b2", shell(`echo b2 >> ${target}/log.txt`));
  };
  await Promise.all([a(), b()]);
  await step("end", shell(`test -f ${target}/go`));
  return "done";
}
"#;

#[test]
fn a_handled_failure_beside_another_branch_resumes() {
    let h = approving_with(Arc::new(MemoryStore::default()));
    let first = h.start(HANDLED_FAILURE_BESIDE_A_SLOW_BRANCH, Value::Null);
    assert!(output(&first).unwrap_err().contains("step 'end' failed"));
    assert_eq!(h.read("log.txt"), "fallback\nb2\n");

    h.write("go", "");
    let second = h.resume(&first, None);
    assert_eq!(output(&second).unwrap(), json!("done"));
    assert_eq!(h.read("log.txt"), "fallback\nb2\n", "neither ran again");
    // Every step is still the one it was.
    let labels: Vec<_> = second.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["a1", "b1", "fallback", "b2", "end"]);
}

#[test]
fn steps_alike_in_label_and_content_take_the_records_in_turn() {
    let h = approving_with(Arc::new(MemoryStore::default()));
    let source = r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  for (let round = 0; round < 3; round++) {
    await step("Mark", shell(`echo x >> ${target}/log.txt`));
  }
  await step("end", shell(`test -f ${target}/go`));
  return "done";
}
"#;
    let first = h.start(source, Value::Null);
    assert!(output(&first).is_err());

    h.write("go", "");
    let second = h.resume(&first, None);
    assert_eq!(output(&second).unwrap(), json!("done"));
    assert_eq!(h.read("log.txt"), "x\nx\nx\n");
    assert_eq!(second.steps.len(), 4);
}

#[test]
fn a_race_between_steps_replays_with_the_same_winner() {
    let h = approving_with(Arc::new(MemoryStore::default()));
    // The step after the race records who won, so the resumed run can be
    // compared with the first whichever step was faster then.
    let source = r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  const winner = await Promise.race([
    step("slow", shell("sleep 0.3")).then(() => "slow"),
    step("fast", shell("true")).then(() => "fast"),
  ]);
  await step("note", shell(`echo ${winner} > ${target}/winner.txt`));
  await step("after", shell(`test -f ${target}/go`));
  return winner;
}
"#;
    let first = h.start(source, Value::Null);
    assert!(output(&first).is_err());
    let winner = h.read("winner.txt").trim().to_string();

    h.write("go", "");
    let second = h.resume(&first, None);
    assert_eq!(output(&second).unwrap(), json!(winner));
}

#[test]
fn canceling_stops_a_running_command_and_what_it_started() {
    let h = approving_with(Arc::new(MemoryStore::default()));
    let stop = h.engine.stop_handle();
    let workflow = h.workflow(
        r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  // The inner shell is a process the command started.
  await step("Wait", shell(`sh -c 'sleep 30; echo late > ${target}/late.txt'`));
}
"#,
    );
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        stop.stop();
    });
    let started = std::time::Instant::now();
    let report = h.start_loaded(workflow, Value::Null, false);
    assert_eq!(report.ending, Ending::Paused);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the command was stopped"
    );
    assert_eq!(report.steps[0].status, StepStatus::Canceled);
    assert!(!h.exists("late.txt"));
}

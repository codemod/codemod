mod common;

use std::sync::Arc;
use std::time::Duration;

use codemod_workflow::events::Event;
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
    assert!(h.events.take().contains(&Event::StepReplayed {
        position: 0,
        label: "Touch".into()
    }));
}

#[test]
fn changing_a_finished_step_before_resuming_is_a_nondeterminism_error() {
    let h = Harness::new();
    h.write("a.txt", "a");

    let first = h.start(&broken_check(), Value::Null);
    let changed = TOUCH_THEN_CHECK.replace("\"*.txt\"", "\"**/*.txt\"");
    let second = h.resume(&first, Some(&changed));

    let error = output(&second).unwrap_err();
    assert!(error.contains("NondeterminismError"), "{error}");
    assert!(error.contains("step #0 ('Touch')"), "{error}");
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
fn workflow_state_is_shared_and_replayed() {
    let h = Harness::new();
    h.write("a.txt", "a");
    let source = r#"
import { step, getState, setState } from "codemod/workflow";
export default async function main() {
  const files = await step("List", () => ["a.txt"]);
  setState("files", files);
  await step("Check", () => { throw new Error("stop"); });
  return getState("files");
}
"#;
    let first = h.start(source, Value::Null);
    assert!(output(&first).is_err());

    let second = h.resume(
        &first,
        Some(&source.replace("throw new Error(\"stop\");", "return 1;")),
    );
    assert_eq!(output(&second).unwrap(), json!(["a.txt"]));
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
    assert!(events.contains(&Event::StepStarted {
        position: 0,
        label: "Touch".into()
    }));
    assert!(events.contains(&Event::StepProgress {
        position: 0,
        processed: 1,
        total: 1
    }));
    assert!(events.iter().any(|e| matches!(
        e,
        Event::StepFinished {
            position: 1,
            status: StepStatus::Succeeded,
            ..
        }
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
    assert_eq!(report.ending, Ending::Canceled);
    assert_eq!(report.run.status, RunStatus::Canceled);
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

    let result = output(&report).unwrap();
    assert_eq!(result["failures"][0]["path"], "a.ts");
    let error = result["failures"][0]["error"].as_str().unwrap();
    assert!(error.contains("TypeError"), "{error}");
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
    record.reopen().unwrap();
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

/// Two branches of two steps each. Positions come from the order steps are
/// issued, which follows the order the first steps finish in; a resume has
/// to settle the recorded steps in that same order or the second steps swap
/// positions.
const TWO_BRANCHES: &str = r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  const a = async () => {
    await step("a1", shell("sleep 0.3"));
    await step("a2", shell(`echo a2 >> ${target}/log.txt`));
  };
  const b = async () => {
    await step("b1", shell("true"));
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
    assert_eq!(report.ending, Ending::Canceled);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the command was stopped"
    );
    assert_eq!(report.steps[0].status, StepStatus::Canceled);
    assert!(!h.exists("late.txt"));
}

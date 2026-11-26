//! Runs whose target is a read-only snapshot a host keeps (Grepkon's
//! storage), not a directory: what they read, what they skip, what they
//! refuse, what they keep from the host, and the per-file cache.

mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use codemod_workflow::events::CollectEvents;
use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::target::{FileCache, FileOutputs, MemoryTarget, TargetFiles};
use codemod_workflow::{
    load_workflow, Ending, Engine, Isolation, MemoryStore, RunReport, Services, StartRequest,
    TestPlan,
};
use serde_json::{json, Value};
use tempfile::TempDir;

/// A path no test creates: a snapshot run must never look there.
fn snapshot_root() -> PathBuf {
    PathBuf::from("/nonexistent/codemod-snapshot")
}

fn moment_snapshot() -> Arc<MemoryTarget> {
    Arc::new(MemoryTarget::new([
        (
            "src/a.ts".to_string(),
            "import m from \"moment\";\nm();\nm();\n".to_string(),
        ),
        ("src/b.ts".to_string(), "export const b = 1;\n".to_string()),
        (
            "src/nested/c.ts".to_string(),
            "import m from \"moment\";\n".to_string(),
        ),
        ("README.md".to_string(), "# moment\n".to_string()),
    ]))
}

#[derive(Default)]
struct CountingCache {
    entries: Mutex<HashMap<[u8; 32], Arc<str>>>,
    hits: AtomicUsize,
    puts: AtomicUsize,
}

impl FileCache for CountingCache {
    fn get(&self, key: &[u8; 32]) -> Option<Arc<str>> {
        let found = self.entries.lock().unwrap().get(key).cloned();
        if found.is_some() {
            self.hits.fetch_add(1, Ordering::SeqCst);
        }
        found
    }

    fn put(&self, key: [u8; 32], outcome: String) {
        self.puts.fetch_add(1, Ordering::SeqCst);
        self.entries.lock().unwrap().insert(key, outcome.into());
    }
}

#[derive(Default)]
struct Outputs(Mutex<Vec<(String, String, Value)>>);

impl FileOutputs for Outputs {
    fn output(&self, _position: u64, label: &str, path: &str, output: &Value) {
        self.0
            .lock()
            .unwrap()
            .push((label.to_string(), path.to_string(), output.clone()));
    }
}

struct Host {
    package: TempDir,
    engine: Engine,
    summary: Arc<codemod_workflow::Summary>,
}

fn host(services: Services) -> Host {
    let summary = Arc::clone(&services.summary);
    Host {
        package: TempDir::new().unwrap(),
        engine: Engine::new(
            Arc::new(MemoryStore::default()),
            Arc::new(CollectEvents::default()),
            Arc::new(NonInteractive { approve_all: true }),
            services,
        ),
        summary,
    }
}

fn isolated() -> Services {
    Services {
        isolation: Some(Isolation {
            memory_limit: 64 << 20,
            max_stack_size: 1 << 20,
        }),
        ..Services::default()
    }
}

impl Host {
    fn file(&self, path: &str, text: &str) {
        common::write(self.package.path(), path, text);
    }

    fn run(
        &self,
        source: &str,
        snapshot: Arc<dyn TargetFiles>,
        test: Option<TestPlan>,
    ) -> RunReport {
        self.file("codemod.yaml", "name: spike\nversion: 0.0.1\n");
        self.file("workflow.ts", source);
        let workflow = load_workflow(&self.package.path().join("workflow.ts")).unwrap();
        self.engine
            .start_on_snapshot(
                StartRequest {
                    run_id: None,
                    workflow,
                    target: snapshot_root(),
                    params: Value::Null,
                    name: None,
                    dry_run: true,
                    test,
                    check_uncommitted: false,
                },
                snapshot,
            )
            .unwrap()
    }
}

fn output(report: &RunReport) -> Result<Value, String> {
    match &report.ending {
        Ending::Succeeded(value) => Ok(value.clone()),
        Ending::Stopped(_) => Ok(Value::Null),
        Ending::Failed(error) => Err(error.clone()),
        other => Err(format!("{other:?}")),
    }
}

fn step_result(report: &RunReport, label: &str) -> Value {
    report
        .steps
        .iter()
        .find(|step| step.label == label)
        .and_then(|step| step.result.clone())
        .unwrap_or(Value::Null)
}

const MIGRATE: &str = r#"
import { step, forEachFile } from "codemod/workflow";
import { parse } from "codemod/ast-grep";

export default async function main() {
  const scan = await step("Find moment imports", forEachFile({ include: "src/**/*.ts", containing: "moment" }, (file) => {
    const root = parse(file).root();
    const calls = root.findAll({ rule: { pattern: "m()" } }).length;
    return { file: file.path, calls };
  }));
  const migrated = await step("Migrate", forEachFile({ include: "src/**/*.ts", containing: "moment" }, (file) => {
    file.write(file.text.replaceAll("moment", "dayjs"));
  }));
  const after = await step("Count dayjs", forEachFile("src/**/*.ts", (file) =>
    file.text.includes("dayjs") ? file.path : undefined));
  return {
    calls: scan.outputs.reduce((sum, row) => sum + row.calls, 0),
    written: migrated.changes.written,
    dayjs: after.outputs,
  };
}
"#;

#[test]
fn a_whole_run_reads_the_snapshot_and_its_changes_stay_in_the_run() {
    let h = host(isolated());

    let report = h.run(MIGRATE, moment_snapshot(), None);

    let diffs: Vec<String> = h
        .summary
        .diffs()
        .iter()
        .map(|diff| diff.path.clone())
        .collect();
    assert_eq!(
        (output(&report).unwrap(), diffs, snapshot_root().exists()),
        (
            json!({
                "calls": 2,
                "written": ["src/a.ts", "src/nested/c.ts"],
                "dayjs": ["src/a.ts", "src/nested/c.ts"],
            }),
            vec!["src/a.ts".to_string(), "src/nested/c.ts".to_string()],
            false
        )
    );
}

#[test]
fn a_step_query_stops_the_run_after_its_step() {
    let h = host(isolated());

    let report = h.run(
        MIGRATE,
        moment_snapshot(),
        Some(TestPlan {
            until: Some("Migrate".to_string()),
            given: Default::default(),
        }),
    );

    let ran: Vec<&str> = report.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(
        (
            ran,
            step_result(&report, "Migrate")["changes"]["written"].clone()
        ),
        (
            vec!["Find moment imports", "Migrate"],
            json!(["src/a.ts", "src/nested/c.ts"])
        )
    );
}

#[test]
fn the_host_is_told_which_file_each_output_came_from() {
    let outputs = Arc::new(Outputs::default());
    let h = host(Services {
        file_outputs: Some(outputs.clone()),
        ..isolated()
    });

    h.run(MIGRATE, moment_snapshot(), None);

    let scan: Vec<(String, Value)> = outputs
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|(label, ..)| label == "Find moment imports")
        .map(|(_, path, output)| (path.clone(), output["calls"].clone()))
        .collect();
    assert_eq!(
        scan,
        vec![
            ("src/a.ts".to_string(), json!(2)),
            ("src/nested/c.ts".to_string(), json!(0))
        ]
    );
}

#[test]
fn unchanged_files_come_from_the_cache_on_the_next_run() {
    let cache = Arc::new(CountingCache::default());
    let h = host(Services {
        file_cache: Some(cache.clone()),
        ..isolated()
    });
    let first = h.run(MIGRATE, moment_snapshot(), None);
    let after_first = (
        cache.hits.load(Ordering::SeqCst),
        cache.puts.load(Ordering::SeqCst),
    );

    let mut files: Vec<(String, String)> = moment_snapshot()
        .paths()
        .iter()
        .map(|path| {
            let text = moment_snapshot().read(&[path]).pop().flatten().unwrap();
            (path.clone(), text.to_string())
        })
        .collect();
    files.push((
        "src/d.ts".to_string(),
        "import m from \"moment\";\nm();\n".to_string(),
    ));
    let second = h.run(MIGRATE, Arc::new(MemoryTarget::new(files)), None);

    assert_eq!(
        (
            after_first,
            output(&first).unwrap()["calls"].clone(),
            output(&second).unwrap()["calls"].clone(),
            cache.hits.load(Ordering::SeqCst),
            cache.puts.load(Ordering::SeqCst) - after_first.1,
        ),
        // First run: 2 + 2 + 3 files run and are kept. Second: only d.ts is
        // new to each step (the third step sees a.ts and c.ts as migrated,
        // as the first run's third step did).
        ((0, 7), json!(2), json!(3), 7, 3)
    );
}

#[test]
fn steps_that_act_outside_the_files_are_skipped_and_the_run_goes_on() {
    let h = host(isolated());

    let report = h.run(
        r#"
import { step, forEachFile, shell, agent, approval, waitForEvent, sleep } from "codemod/workflow";
export default async function main() {
  const format = await step("Format", shell`npx prettier --write src`);
  const review = await step("Review", agent("Review the migration"));
  const ok = await step("Approve", approval("Apply?"));
  const event = await waitForEvent("merged");
  await sleep("1h");
  const files = await step("Files", forEachFile("src/**/*.ts", (file) => file.path));
  return { format, review, ok, event, files: files.outputs.length };
}
"#,
        moment_snapshot(),
        None,
    );

    assert_eq!(
        output(&report).unwrap(),
        json!({
            "format": { "skipped": true },
            "review": { "skipped": true },
            "ok": { "approved": true },
            "event": { "skipped": true },
            "files": 3,
        })
    );
}

#[test]
fn tasks_that_read_the_disk_refuse_to_run_on_a_snapshot() {
    let h = host(isolated());
    let error = |task: &str| {
        let report = h.run(
            &format!(
                r#"
import {{ step, forEachFile, shard }} from "codemod/workflow";
import {{ dependencyGraph }} from "codemod/deps";
export default async function main() {{ return step({task}); }}
"#
            ),
            moment_snapshot(),
            None,
        );
        output(&report).unwrap_err()
    };

    let errors = [
        error("dependencyGraph((g) => g.packages.length)"),
        error(
            r#"forEachFile({ include: "src/**/*.ts", semanticAnalysis: "workspace" }, (f) => f.path)"#,
        ),
    ];
    assert!(
        errors
            .iter()
            .all(|e| e.contains("does not run on a snapshot")),
        "{errors:?}"
    );
}

#[test]
fn ast_grep_rules_select_and_read_snapshot_files() {
    let h = host(isolated());

    let report = h.run(
        r#"
import { step, forEachFile, astGrep } from "codemod/workflow";
export default async function main() {
  const result = await step("Rule", forEachFile("src/**", astGrep({ id: "r", language: "typescript", rule: { pattern: "m()" }, fix: "dayjs()" })));
  return result.changes.written;
}
"#,
        moment_snapshot(),
        None,
    );

    assert_eq!(output(&report).unwrap(), json!(["src/a.ts"]));
}

#[test]
fn isolated_callbacks_see_no_host_environment() {
    let h = host(isolated());
    std::env::set_var("SPIKE_SECRET", "s3cret");

    let report = h.run(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const seen = await step("Env", forEachFile("src/b.ts", () => ({
    env: Object.keys(globalThis.process?.env ?? {}).length,
    cwd: globalThis.process?.cwd?.() ?? null,
  })));
  return seen.outputs[0];
}
"#,
        moment_snapshot(),
        None,
    );

    assert_eq!(
        output(&report).unwrap(),
        json!({ "env": 0, "cwd": "/nonexistent/codemod-snapshot" })
    );
}

#[test]
fn isolated_callbacks_cannot_import_the_host_os_module() {
    let h = host(isolated());

    let report = h.run(
        r#"
import { step, forEachFile } from "codemod/workflow";
import os from "os";
export default async function main() {
  return step("Os", forEachFile("src/b.ts", () => os.hostname()));
}
"#,
        moment_snapshot(),
        None,
    );

    let error = output(&report).unwrap_err();
    assert!(error.contains("os"), "{error}");
}

#[test]
fn isolated_imports_cannot_leave_the_package() {
    let h = host(isolated());
    let outside = TempDir::new().unwrap();
    common::write(outside.path(), "secret.ts", "export default \"leaked\";\n");
    let secret = outside.path().join("secret.ts");

    let report = h.run(
        &format!(
            r#"
import {{ step }} from "codemod/workflow";
export default async function main() {{
  return step("Read", async () => (await import({:?})).default);
}}
"#,
            secret.to_string_lossy()
        ),
        moment_snapshot(),
        None,
    );

    let error = output(&report).unwrap_err();
    assert!(error.contains("inside the package"), "{error}");
}

#[test]
fn an_isolated_callback_that_allocates_too_much_fails_its_file() {
    let h = host(isolated());

    let report = h.run(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  const r = await step("Grow", forEachFile("src/**/*.ts", (file) => {
    if (file.path !== "src/b.ts") return file.path;
    const hog = [];
    while (true) hog.push("x".repeat(1 << 20));
  }));
  return r.outputs;
}
"#,
        moment_snapshot(),
        None,
    );

    let grow = report.steps.iter().find(|s| s.label == "Grow").unwrap();
    assert_eq!(
        (output(&report).unwrap(), grow.failed_files()),
        (json!(["src/a.ts", "src/nested/c.ts"]), 1)
    );
}

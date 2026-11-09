//! `codemod workflow run -w workflow.ts`: the orchestration engine end to
//! end, from the TypeScript workflow to the edited target, in one process.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};
use tempfile::TempDir;

/// The target every run starts from.
const FILES: [(&str, &str); 4] = [
    ("a.ts", "oldApi(1);\n"),
    ("b.ts", "export const b = 1;\n"),
    ("c.ts", "export const c = oldApi(2);\n"),
    ("sub/d.old.ts", "export const d = 3;\n"),
];

/// `FILES` after the fixtures' `replace` and `rename` transforms.
const MIGRATED: [(&str, &str); 4] = [
    ("a.ts", "newApi(1);\n"),
    ("b.ts", "export const b = 1;\n"),
    ("c.ts", "export const c = newApi(2);\n"),
    ("sub/d.new.ts", "export const d = 3;\n"),
];

fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/orchestration/tests/fixtures")
        .join(path)
}

struct Run {
    scratch: TempDir,
}

impl Run {
    fn new() -> Self {
        let run = Self {
            scratch: TempDir::new().unwrap(),
        };
        for (path, content) in FILES {
            let file = run.target().join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, content).unwrap();
        }
        run
    }

    fn target(&self) -> PathBuf {
        self.scratch.path().join("repo")
    }

    fn codemod(&self, workflow: &Path, extra: &[&str]) -> Output {
        let home = self.scratch.path().join("home");
        Command::new(env!("CARGO_BIN_EXE_codemod"))
            .arg("--disable-analytics")
            .args(["workflow", "run", "-w"])
            .arg(workflow)
            .arg("--target")
            .arg(self.target())
            .args(extra)
            .env("HOME", &home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("CODEMOD_WORKFLOW_CACHE", self.scratch.path().join("cache"))
            .env("NO_COLOR", "1")
            .output()
            .expect("failed to run codemod")
    }

    fn files(&self) -> BTreeMap<String, String> {
        let mut files = BTreeMap::new();
        let mut pending = vec![self.target()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    let relative = path.strip_prefix(self.target()).unwrap();
                    files.insert(
                        relative.to_string_lossy().into_owned(),
                        fs::read_to_string(&path).unwrap(),
                    );
                }
            }
        }
        files
    }
}

fn tree(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(path, content)| (path.to_string(), content.to_string()))
        .collect()
}

fn output(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn stderr(output: &Output) -> String {
    assert!(!output.status.success(), "the run succeeded");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn a_jssg_sequence_edits_and_renames_files_in_the_target() {
    let run = Run::new();
    run.codemod(&fixture("quickjs/jssg-sequence.ts"), &[]);
    assert_eq!(run.files(), tree(&MIGRATED));
}

#[test]
fn the_workflow_output_is_printed_as_json() {
    let run = Run::new();
    let printed = output(&run.codemod(&fixture("quickjs/jssg-sequence.ts"), &[]));
    assert_eq!(printed, json!([]));
}

macro_rules! input_reaches_the_root {
    ($($name:ident: $text:literal => $value:tt,)*) => {$(
        #[test]
        fn $name() {
            let run = Run::new();
            let printed = output(&run.codemod(&fixture("load/input.ts"), &["--input", $text]));
            assert_eq!(printed, json!({ "received": $value }));
        }
    )*};
}

input_reaches_the_root! {
    null_input_reaches_the_root: "null" => null,
    object_input_reaches_the_root: r#"{"name":"x"}"# => { "name": "x" },
}

#[test]
fn a_root_that_declares_input_requires_it() {
    let run = Run::new();
    let message = stderr(&run.codemod(&fixture("load/input.ts"), &[]));
    assert!(
        message.contains("workflow requires an input value; pass --input <json>"),
        "{message}"
    );
}

#[test]
fn input_that_is_not_json_is_refused() {
    let run = Run::new();
    let message = stderr(&run.codemod(&fixture("load/input.ts"), &["--input", "{name}"]));
    assert!(message.contains("--input must be valid JSON"), "{message}");
}

macro_rules! a_failing_jssg_run_writes_nothing {
    ($($name:ident: $which:literal => $message:literal,)*) => {$(
        #[test]
        fn $name() {
            let run = Run::new();
            let input = format!("{:?}", $which);
            let message = stderr(&run.codemod(&fixture("quickjs/jssg-failures.ts"), &["--input", &input]));
            assert!(message.contains($message), "{message}");
            assert_eq!(run.files(), tree(&FILES));
        }
    )*};
}

a_failing_jssg_run_writes_nothing! {
    a_transform_that_throws_fails_the_run_and_writes_nothing: "fail-second" => "second file exploded",
    a_rename_conflict_fails_the_run_and_writes_nothing: "conflict" => "same.ts",
    a_rename_outside_the_target_fails_the_run_and_writes_nothing: "escape" => "is outside the target directory",
}

#[test]
fn a_failed_run_resumes_from_its_history_without_rerunning_finished_steps() {
    let run = Run::new();
    let history = run.scratch.path().join("history.json");
    let history = history.to_str().unwrap();
    let workflow = fixture("quickjs/shell-resume.ts");
    stderr(&run.codemod(&workflow, &["--history", history, "--input", "true"]));
    output(&run.codemod(&workflow, &["--history", history, "--input", "false"]));
    let log = fs::read_to_string(run.target().join("runs.log")).unwrap();
    assert_eq!(log, "count\ngate\n");
}

#[test]
fn a_node_import_fails_the_build() {
    let run = Run::new();
    let workflow = run.scratch.path().join("node-import.ts");
    fs::write(
        &workflow,
        "import { readFileSync } from \"node:fs\";\nexport default readFileSync;\n",
    )
    .unwrap();
    let message = stderr(&run.codemod(&workflow, &[]));
    assert!(message.contains("is a Node module"), "{message}");
}

//! `codemod workflow run -w workflow.ts`: the orchestration engine end to
//! end, from the TypeScript workflow to the edited target, in one process.
//! The agent stand-in is a Node script; the decision API stub runs in the
//! test process.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use support::{alive, files, fixture, output, signal, stderr, wait_for, Scratch};

/// The target every run starts from.
fn initial() -> BTreeMap<String, String> {
    files(&[
        ("a.ts", "oldApi(1);\n"),
        ("b.ts", "export const b = 1;\n"),
        ("c.ts", "export const c = oldApi(2);\n"),
        ("sub/d.old.ts", "export const d = 3;\n"),
    ])
}

/// `initial()` after the fixtures' `replace` and `rename` transforms.
fn migrated() -> BTreeMap<String, String> {
    files(&[
        ("a.ts", "newApi(1);\n"),
        ("b.ts", "export const b = 1;\n"),
        ("c.ts", "export const c = newApi(2);\n"),
        ("sub/d.new.ts", "export const d = 3;\n"),
    ])
}

fn quickjs(name: &str) -> PathBuf {
    fixture(&format!("quickjs/{name}"))
}

#[test]
fn a_jssg_sequence_edits_and_renames_files_in_the_target() {
    let scratch = Scratch::with(&initial());
    scratch.run(&quickjs("jssg-sequence.ts"), &[]);
    assert_eq!(scratch.files(), migrated());
}

#[test]
fn the_workflow_output_is_printed_as_json() {
    let scratch = Scratch::with(&initial());
    let printed = output(&scratch.run(&quickjs("jssg-sequence.ts"), &[]));
    assert_eq!(printed, json!([]));
}

#[test]
fn dynamic_bodies_run_concurrent_commands_invocation_targets_and_a_shell_step() {
    let scratch = Scratch::with(&initial());
    let printed = output(&scratch.run(&quickjs("jssg-dynamic.ts"), &[]));
    assert_eq!(
        printed,
        json!({
            "first": [{ "file": "a.ts" }],
            "second": [],
            "third": [],
            "stdout": "./a.ts\n./b.ts\n./c.ts\n./sub/d.new.ts\nnewApi(1);\n",
        })
    );
}

#[test]
fn a_transform_in_an_imported_module_is_split_into_an_artifact() {
    let scratch = Scratch::with(&initial());
    let built = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(orchestration_build::build_file(&fixture(
            "load/definitions.ts",
        )))
        .unwrap();
    let printed = output(&scratch.run(&fixture("load/dynamic.ts"), &[]));
    assert_eq!(
        printed["transform"],
        json!({ "name": "migrate", "hash": built.artifacts[0].hash })
    );
}

#[test]
fn a_workflow_that_imports_the_package_runs_on_the_built_in_runtime() {
    let scratch = Scratch::with(&initial());
    let workflow = scratch.path("package-import.ts");
    fs::write(
        &workflow,
        "import { shell } from \"@codemod.com/orchestration\";\nexport default shell({ name: \"hello\", command: \"echo hello\" });\n",
    )
    .unwrap();
    let printed = output(&scratch.bare_command(&workflow, &[]).output().unwrap());
    assert_eq!(printed["stdout"], json!("hello\n"));
}

#[test]
fn a_node_import_fails_the_build() {
    let scratch = Scratch::with(&initial());
    let workflow = scratch.path("node-import.ts");
    fs::write(
        &workflow,
        "import { readFileSync } from \"node:fs\";\nexport default readFileSync;\n",
    )
    .unwrap();
    let message = stderr(&scratch.run(&workflow, &[]));
    assert!(message.contains("is a Node module"), "{message}");
}

macro_rules! input_reaches_the_root {
    ($($name:ident: $text:literal => $value:tt,)*) => {$(
        #[test]
        fn $name() {
            let scratch = Scratch::with(&initial());
            let printed = output(&scratch.run(&fixture("load/input.ts"), &["--input", $text]));
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
    let scratch = Scratch::with(&initial());
    let message = stderr(&scratch.run(&fixture("load/input.ts"), &[]));
    assert!(
        message.contains("workflow requires an input value; pass --input <json>"),
        "{message}"
    );
}

#[test]
fn input_that_is_not_json_is_refused() {
    let scratch = Scratch::with(&initial());
    let message = stderr(&scratch.run(&fixture("load/input.ts"), &["--input", "{name}"]));
    assert!(message.contains("--input must be valid JSON"), "{message}");
}

macro_rules! a_failing_jssg_run_writes_nothing {
    ($($name:ident: $which:literal => $message:literal,)*) => {$(
        #[test]
        fn $name() {
            let scratch = Scratch::with(&initial());
            let input = format!("{:?}", $which);
            let message = stderr(&scratch.run(&quickjs("jssg-failures.ts"), &["--input", &input]));
            assert!(message.contains($message), "{message}");
            assert_eq!(scratch.files(), initial());
        }
    )*};
}

a_failing_jssg_run_writes_nothing! {
    a_transform_that_throws_fails_the_run_and_writes_nothing: "fail-second" => "second file exploded",
    a_rename_conflict_fails_the_run_and_writes_nothing: "conflict" => "same.ts",
    a_rename_outside_the_target_fails_the_run_and_writes_nothing: "escape" => "is outside the target directory",
}

/// Ignore files at several levels, a negation, a parent rule a negation
/// cannot undo, and a nested `.ignore`, around the usual files.
fn ignored() -> BTreeMap<String, String> {
    let mut all = initial();
    all.extend(files(&[
        (".gitignore", "*.gen.ts\nvendor/\n!keep.gen.ts\n"),
        ("keep.gen.ts", "oldApi(5);\n"),
        ("x.gen.ts", "oldApi(6);\n"),
        ("vendor/v.ts", "oldApi(7);\n"),
        ("sub/.ignore", "!*.gen.ts\n"),
        ("sub/y.gen.ts", "oldApi(8);\n"),
        ("sub/deeper/.gitignore", "z.ts\n"),
        ("sub/deeper/z.ts", "oldApi(9);\n"),
        ("sub/deeper/w.ts", "oldApi(10);\n"),
    ]));
    all
}

macro_rules! native_file_selection_matches_the_interpreted_walker {
    ($($name:ident: $workflow:literal,)*) => {$(
        #[test]
        fn $name() {
            let run = |native: &str| {
                let scratch = Scratch::with(&ignored());
                let mut command = scratch.command(&quickjs($workflow), &[]);
                let printed = output(&command.env("CODEMOD_NATIVE_SELECT", native).output().unwrap());
                (printed, scratch.files())
            };
            assert_eq!(run("1"), run("0"));
        }
    )*};
}

native_file_selection_matches_the_interpreted_walker! {
    a_sequence_selects_the_same_files_natively: "jssg-sequence.ts",
    a_dynamic_body_selects_the_same_files_natively: "jssg-dynamic.ts",
}

/// The shell-resume fixture: `count`, then a crash with input `true`, else
/// `gate`. Both steps append to runs.log.
struct Resume {
    scratch: Scratch,
}

impl Resume {
    fn new() -> Self {
        Self {
            scratch: Scratch::with(&initial()),
        }
    }

    fn run(&self, crash: &str) -> Output {
        let history = self.scratch.path("history.json");
        self.scratch.run(
            &quickjs("shell-resume.ts"),
            &["--history", history.to_str().unwrap(), "--input", crash],
        )
    }

    fn log(&self) -> String {
        fs::read_to_string(self.scratch.target().join("runs.log")).unwrap()
    }

    fn scheduled(&self) -> Vec<Value> {
        let history = fs::read_to_string(self.scratch.path("history.json")).unwrap();
        scheduled(&serde_json::from_str(&history).unwrap())
            .into_iter()
            .map(|command| command["id"].clone())
            .collect()
    }
}

fn scheduled(history: &Value) -> Vec<&Value> {
    history["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == "scheduled")
        .map(|event| &event["command"])
        .collect()
}

#[test]
fn a_failed_run_resumes_from_its_history_without_rerunning_finished_steps() {
    let resume = Resume::new();
    stderr(&resume.run("true"));
    output(&resume.run("false"));
    assert_eq!(resume.log(), "count\ngate\n");
}

#[test]
fn a_finished_run_replays_its_output_without_running_anything() {
    let resume = Resume::new();
    resume.run("true");
    resume.run("false");
    let replayed = output(&resume.run("false"));
    assert_eq!(
        (replayed, resume.log()),
        (json!("passed\n"), "count\ngate\n".to_owned())
    );
}

#[test]
fn a_resumed_history_schedules_each_command_once() {
    let resume = Resume::new();
    resume.run("true");
    resume.run("false");
    assert_eq!(resume.scheduled(), [json!("count"), json!("gate")]);
}

fn parallel_dynamic() -> (Value, Value) {
    let scratch = Scratch::with(&initial());
    let history = scratch.path("history.json");
    let printed = output(&scratch.run(
        &quickjs("parallel-dynamic.ts"),
        &["--history", history.to_str().unwrap()],
    ));
    let recorded = serde_json::from_str(&fs::read_to_string(history).unwrap()).unwrap();
    (printed, recorded)
}

#[test]
fn parallel_dynamic_bodies_return_their_outputs() {
    let (printed, _) = parallel_dynamic();
    assert_eq!(printed, json!(["a2\n", ["b1\n", "c1\n", "b2\n"]]));
}

#[test]
fn commands_issued_after_awaits_in_parallel_bodies_are_concurrent() {
    // As with AsyncLocalStorage: without propagation they would lose the flag.
    let (_, history) = parallel_dynamic();
    let mut concurrent: Vec<&str> = scheduled(&history)
        .into_iter()
        .filter(|command| command["concurrent"] == true)
        .map(|command| command["id"].as_str().unwrap())
        .collect();
    concurrent.sort();
    assert_eq!(concurrent, ["a1", "a2", "b1", "b2", "c1"]);
}

#[test]
fn the_async_context_matches_async_local_storage_across_awaits_groups_and_generators() {
    let scratch = Scratch::with(&initial());
    let seen = output(&scratch.run(&quickjs("async-context.ts"), &[]));
    // What Node's AsyncLocalStorage observes on the same scenario.
    let als = json!({
        "top": "-", "A0": "A", "A1": "A", "B0": "B", "C0": "C", "B1": "B", "C1": "C",
        "B2": "B", "Binner": "B.inner", "B3": "B", "A2": "A", "Dthen": "D", "A3": "A",
        "E1": "E", "A4": "A", "F1": "F", "F2": "F", "F3": "F", "Acatch": "A",
        "Afinally": "A", "Athen": "A", "Hawait": "H", "Hresolve": "H", "HfromThen": "H",
        "HfromAsync": "H", "Hall": "H", "Hexecutor": "H", "end": "-",
    });
    assert_eq!(seen, als);
}

fn semantic() -> (Value, BTreeMap<String, String>) {
    let scratch = Scratch::with(&files(&[
        (
            "main.ts",
            "import { add } from \"./utils\";\nconst result = add(1, 2);\n",
        ),
        (
            "utils.ts",
            "export function add(a: number, b: number): number {\n  return a + b;\n}\n",
        ),
    ]));
    let printed = output(&scratch.run(&quickjs("semantic.ts"), &[]));
    (printed, scratch.files())
}

#[test]
fn workspace_semantic_analysis_finds_a_definition_in_another_file() {
    let (printed, _) = semantic();
    assert_eq!(
        printed[0],
        json!([{ "file": "main.ts", "definition": "utils.ts" }])
    );
}

#[test]
fn workspace_semantic_analysis_stages_a_cross_file_edit() {
    let (_, files) = semantic();
    assert!(
        files["utils.ts"].contains("function sum"),
        "{}",
        files["utils.ts"]
    );
}

#[test]
fn file_semantic_analysis_classifies_a_local_definition() {
    let (printed, _) = semantic();
    let main = printed[1]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["file"] == "main.ts")
        .unwrap();
    assert_eq!(main["kind"], json!("local"));
}

/// The stand-in for `codemod-agent`.
fn fake_agent() -> PathBuf {
    fixture("fake-agent.mjs")
}

/// What the fake agent saw: its request, environment and secrets.
fn agent_echo() -> Value {
    let scratch = Scratch::with(&initial());
    let mut command = scratch.command(&quickjs("static-agent.ts"), &[]);
    command
        .env("CODEMOD_AGENT_BIN", fake_agent())
        .env("LLM_API_KEY", "llm-key")
        .env("DECISION_API_KEY", "decision-key");
    let printed = output(&command.output().unwrap());
    serde_json::from_str(printed["text"].as_str().unwrap()).unwrap()
}

#[test]
fn an_agent_step_reaches_the_runner_as_an_agent_request() {
    let echo = agent_echo();
    assert_eq!(
        (
            &echo["request"]["commandId"],
            &echo["request"]["operation"]["kind"]
        ),
        (&json!("fix"), &json!("agent"))
    );
}

#[test]
fn the_runner_gets_the_model_key_on_stdin() {
    assert_eq!(agent_echo()["secrets"], json!({ "LLM_API_KEY": "llm-key" }));
}

#[test]
fn the_runner_environment_holds_no_keys() {
    let echo = agent_echo();
    let env = echo["env"].as_object().unwrap();
    assert_eq!(
        (
            env.get("CODEMOD_AGENT_SECRETS"),
            env.contains_key("LLM_API_KEY"),
            env.contains_key("DECISION_API_KEY"),
        ),
        (Some(&json!("stdin")), false, false)
    );
}

#[test]
fn an_agent_step_without_the_runner_fails() {
    let scratch = Scratch::with(&initial());
    let mut command = scratch.command(&quickjs("static-agent.ts"), &[]);
    command.env("CODEMOD_AGENT_BIN", scratch.path("no-such-agent"));
    let message = stderr(&command.output().unwrap());
    assert!(
        message.contains("agent steps need the codemod-agent runner"),
        "{message}"
    );
}

/// Starts `command` and waits until `file` in the target has content.
fn started(scratch: &Scratch, command: Command, file: &str) -> Child {
    let child = scratch.spawn(command);
    let path = scratch.target().join(file);
    wait_for(&format!("{file} written"), || {
        fs::read_to_string(&path).is_ok_and(|text| !text.trim().is_empty())
    });
    child
}

#[test]
fn sigterm_during_an_agent_step_kills_the_runners_whole_process_tree() {
    let scratch = Scratch::with(&initial());
    let pid_file = scratch.target().join("pids.json");
    fs::write(
        scratch.target().join("fake-agent.json"),
        json!({
            "FAKE_AGENT_MODE": "hang",
            "FAKE_PID_FILE": pid_file,
            "FAKE_CHILDREN": "2",
            "FAKE_DETACH_CHILDREN": "1",
        })
        .to_string(),
    )
    .unwrap();
    let mut command = scratch.command(&quickjs("static-agent.ts"), &[]);
    command.env("CODEMOD_AGENT_BIN", fake_agent());
    let mut child = started(&scratch, command, "pids.json");
    let pids: Vec<i64> = serde_json::from_str(&fs::read_to_string(&pid_file).unwrap()).unwrap();
    signal(&child, libc::SIGTERM);
    child.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while pids.iter().any(|&pid| alive(pid)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let survivors: Vec<i64> = pids.into_iter().filter(|&pid| alive(pid)).collect();
    assert_eq!(survivors, Vec::<i64>::new());
}

#[test]
fn sigterm_kills_the_running_shell_and_fails_the_run_promptly() {
    let scratch = Scratch::with(&initial());
    let command = scratch.command(&quickjs("shell-hang.ts"), &[]);
    let mut child = started(&scratch, command, "hang.pid");
    let pid: i64 = fs::read_to_string(scratch.target().join("hang.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let signalled = Instant::now();
    signal(&child, libc::SIGTERM);
    let status = child.wait().unwrap();
    assert_eq!(
        (
            status.success(),
            signalled.elapsed() < Duration::from_secs(5),
            alive(pid)
        ),
        (false, true, false)
    );
}

#[test]
fn sigint_cancels_a_running_jssg_batch_promptly_and_writes_nothing() {
    // 300 files at ~10 ms each: about 3 s of transforms if the signal were ignored.
    let mut many = initial();
    for i in 0..300 {
        many.insert(format!("many/f{i}.ts"), format!("oldApi({i});\n"));
    }
    let scratch = Scratch::with(&many);
    let command = scratch.command(&quickjs("jssg-long.ts"), &[]);
    let child = started(&scratch, command, "started.marker");
    // Well inside the batch.
    std::thread::sleep(Duration::from_millis(300));
    let signalled = Instant::now();
    signal(&child, libc::SIGINT);
    let result = child.wait_with_output().unwrap();
    let elapsed = signalled.elapsed();
    let message = stderr(&result);
    let mut after = scratch.files();
    after.remove("started.marker");
    // At most one 64-file chunk (~0.65 s) plus the file in flight.
    assert_eq!(
        (
            elapsed < Duration::from_millis(1500),
            message.contains("command 'slow' cancelled"),
            after == many,
        ),
        (true, true, true),
        "{elapsed:?} {message}"
    );
}

/// Each request's path and lowercase headers.
type Requests = Mutex<Vec<(String, BTreeMap<String, String>)>>;

/// A decision API stub: answers System One and OpenAI-shaped requests and
/// records each request's path and headers.
struct DecisionStub {
    port: u16,
    requests: Arc<Requests>,
    scratch: Scratch,
}

impl DecisionStub {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve(stream, &log));
            }
        });
        let scratch = Scratch::with(&initial());
        Self {
            port,
            requests,
            scratch,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn run(&self, workflow: &str, input: Option<&str>, env: &[(&str, &str)]) -> Value {
        let input = input.map(|url| json!(url).to_string());
        let extra: Vec<&str> = input.iter().flat_map(|input| ["--input", input]).collect();
        let mut command = self.scratch.command(&quickjs(workflow), &extra);
        command.envs(env.iter().copied());
        output(&command.output().unwrap())
    }

    /// The path and the `header` of every request.
    fn requests(&self, header: &str) -> Vec<(String, Value)> {
        let requests = self.requests.lock().unwrap();
        let value = |headers: &BTreeMap<String, String>| json!(headers.get(header));
        requests
            .iter()
            .map(|(path, headers)| (path.clone(), value(headers)))
            .collect()
    }
}

/// Answers one request on `stream`, which then closes.
fn serve(stream: TcpStream, log: &Requests) {
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let path = line.split(' ').nth(1).unwrap().to_owned();
    let mut headers = BTreeMap::new();
    while let Some((name, value)) = {
        line.clear();
        reader.read_line(&mut line).unwrap();
        line.trim_end().split_once(": ")
    } {
        headers.insert(name.to_ascii_lowercase(), value.to_owned());
    }
    let length = headers
        .get("content-length")
        .map_or(0, |n| n.parse().unwrap());
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let questions = serde_json::from_slice::<Value>(&body).unwrap_or_default()["questions"].take();
    let answer = answer(&path, &questions);
    log.lock().unwrap().push((path, headers));
    let (status, body) = match answer {
        Some(answer) => ("200 OK", answer),
        None => (
            "404 Not Found",
            json!({ "error": { "message": "not found" } }),
        ),
    };
    let body = body.to_string();
    let head = format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
    (&stream)
        .write_all(format!("{head}{body}").as_bytes())
        .unwrap();
}

/// A yes/no probability of 0.25 for every question, in the provider's shape.
fn answer(path: &str, questions: &Value) -> Option<Value> {
    if path.ends_with("/systemone") {
        let answers: serde_json::Map<String, Value> = (questions.as_object()?.keys())
            .map(|id| (id.clone(), json!({ "type": "noul", "noul": 0.25 })))
            .collect();
        let usage = json!({ "input_tokens": 3, "output_tokens": 1 });
        Some(json!({ "model": "stub-1", "answers": answers, "usage": usage }))
    } else if path.ends_with("/decisions") {
        let answers: Vec<Value> = (questions.as_array()?.iter())
            .map(|q| json!({ "type": "predicate", "name": q["name"], "probability": 0.25 }))
            .collect();
        Some(json!({ "model": "stub-luna", "answers": answers, "usage": { "input_tokens": 5 } }))
    } else {
        None
    }
}

fn judged(provider: &str, model: &str, usage: Value) -> Value {
    json!(["a.ts", "b.ts", "c.ts", "sub/d.old.ts"].map(|file| json!({
        "file": file,
        "assessment": {
            "provider": provider,
            "model": model,
            "calibrated": true,
            "answers": { "risky": { "type": "yesno", "yes": 0.25 } },
            "usage": usage,
        },
    })))
}

fn system_one(stub: &DecisionStub) -> Value {
    let base = stub.url("");
    stub.run(
        "assessment.ts",
        None,
        &[
            ("TYPESAFE_API_KEY", "test-key"),
            ("TYPESAFE_BASE_URL", &base),
        ],
    )
}

#[test]
fn an_assessment_asks_system_one_per_file_configured_by_typesafe_variables() {
    let stub = DecisionStub::start();
    assert_eq!(
        system_one(&stub),
        judged(
            "systemone",
            "stub-1",
            json!({ "inputTokens": 3, "outputTokens": 1 })
        )
    );
}

#[test]
fn system_one_requests_carry_the_key_as_a_bearer_token() {
    let stub = DecisionStub::start();
    system_one(&stub);
    assert_eq!(
        stub.requests("authorization"),
        vec![("/v1/systemone".to_owned(), json!("Bearer test-key")); 4]
    );
}

#[test]
fn an_assessment_asks_openai_decisions_when_the_provider_is_openai() {
    let stub = DecisionStub::start();
    let base = stub.url("");
    let printed = stub.run(
        "assessment.ts",
        None,
        &[
            ("DECISION_PROVIDER", "openai"),
            ("DECISION_BASE_URL", &base),
            ("DECISION_API_KEY", "openai-key"),
        ],
    );
    assert_eq!(
        (printed, stub.requests("authorization")),
        (
            judged("openai", "stub-luna", json!({ "inputTokens": 5 })),
            vec![("/v1/decisions".to_owned(), json!("Bearer openai-key")); 4]
        )
    );
}

#[test]
fn the_raw_key_goes_in_the_configured_header_on_the_configured_path() {
    let stub = DecisionStub::start();
    let base = stub.url("/foundry");
    stub.run(
        "assessment.ts",
        None,
        &[
            ("DECISION_BASE_URL", &base),
            ("DECISION_PATH", "/providers/microsoft/v1/systemone"),
            ("DECISION_AUTH_HEADER", "api-key"),
            ("DECISION_API_KEY", "azure-key"),
        ],
    );
    assert_eq!(
        stub.requests("api-key"),
        vec![
            (
                "/foundry/providers/microsoft/v1/systemone".to_owned(),
                json!("azure-key")
            );
            4
        ]
    );
}

macro_rules! workflow_code_reaches_only_the_decision_api_root {
    ($($name:ident: $url:expr => $expected:literal,)*) => {$(
        #[test]
        fn $name() {
            let stub = DecisionStub::start();
            let root = stub.url("/v1");
            let url: fn(&DecisionStub) -> String = $url;
            let printed = stub.run("http-escape.ts", Some(&url(&stub)), &[("DECISION_BASE_URL", &root)]);
            assert_eq!(printed, json!($expected));
        }
    )*};
}

workflow_code_reaches_only_the_decision_api_root! {
    workflow_code_cannot_reach_another_path: |stub| stub.url("/other") => "EACCES",
    workflow_code_cannot_reach_another_host: |stub| format!("http://localhost:{}/v1/systemone", stub.port) => "EACCES",
    workflow_code_cannot_reach_another_origin: |_| "https://example.com/v1/systemone".to_owned() => "EACCES",
    workflow_code_reaches_a_path_under_the_root: |stub| stub.url("/v1/models") => "reached",
}

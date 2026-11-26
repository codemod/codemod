//! Callbacks are pure, and `http()` is how a workflow reaches the network:
//! a declared step whose response is recorded and replayed on resume.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use codemod_workflow::events::Event;
use codemod_workflow::load_workflow;
use common::{output, Harness};
use serde_json::{json, Value};

/// Load a workflow that should fail to build; returns the message.
fn load_error(h: &Harness, source: &str) -> String {
    let path = h.bundle.path().join("workflow.ts");
    std::fs::write(&path, source).unwrap();
    match load_workflow(&path) {
        Ok(_) => panic!("expected the workflow not to load"),
        Err(error) => error.to_string(),
    }
}

/// A local HTTP server answering every request with `status` and a JSON
/// body that echoes the request; counts the requests and keeps them.
struct Server {
    url: String,
    hits: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
}

fn server(status: u16) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (counter, kept) = (Arc::clone(&hits), Arc::clone(&requests));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                head.push_str(&line);
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let request = format!("{head}\r\n{}", String::from_utf8_lossy(&body));
            counter.fetch_add(1, Ordering::SeqCst);
            kept.lock().unwrap().push(request.clone());
            let first = request.lines().next().unwrap_or_default().to_string();
            let answer =
                json!({ "line": first, "body": String::from_utf8_lossy(&body) }).to_string();
            let reason = if status < 400 { "OK" } else { "Server Error" };
            let _ = write!(
                stream,
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    Server {
        url,
        hits,
        requests,
    }
}

// ------------------------------------------------------- pure callbacks ---

/// What a callback may not use fails at run time with the step to use
/// instead (`globalThis.` gets past the build's check).
#[test]
fn callbacks_cannot_reach_the_network_processes_or_other_files() {
    let cases = [
        (
            "await globalThis.fetch(\"https://example.com\")",
            "fetch is not available in workflow callbacks",
            "http(url, options)",
        ),
        (
            "globalThis.setTimeout(() => {}, 1)",
            "setTimeout is not available in workflow callbacks",
            "sleep(",
        ),
        (
            "(await import(\"child_process\")).execSync(\"echo hi\")",
            "child_process.execSync is not available in workflow callbacks",
            "shell(\"command\")",
        ),
        (
            "(await import(\"fs\")).writeFileSync(\"other.txt\", \"x\")",
            "fs is not available in 2.0 workflows",
            "codemod/files",
        ),
        (
            "{ process.env.FOO = \"bar\"; }",
            "process.env cannot be changed in workflow callbacks",
            "shell(command, { env })",
        ),
    ];
    for (code, message, hint) in cases {
        let h = Harness::new();
        h.write("a.txt", "a\n");
        let source = format!(
            r#"
import {{ step }} from "codemod/workflow";
export default async function main() {{
  return step("Side effect", async () => {{ {code}; return "ran"; }});
}}
"#
        );
        let error = output(&h.start(&source, Value::Null)).unwrap_err();
        assert!(error.contains(message), "{code}: {error}");
        assert!(error.contains(hint), "{code}: {error}");
        assert!(!h.exists("other.txt"));
    }
}

/// The same in a `forEachFile` callback: the file fails, with the hint.
#[test]
fn a_for_each_file_callback_that_fetches_fails_its_file_with_the_hint() {
    let h = Harness::new();
    h.write("a.txt", "a\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  return step(forEachFile("*.txt", async (file) => {
    await globalThis.fetch("https://example.com");
    file.write("changed");
  }));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("fetch is not available in workflow callbacks"),
        "{error}"
    );
    assert!(error.contains("http(url, options)"), "{error}");
    assert_eq!(h.read("a.txt"), "a\n");
}

/// Callbacks can read and create target files without a capability.
#[test]
fn callbacks_read_and_create_target_files() {
    let h = Harness::new();
    h.write("a.txt", "a\n");
    h.write("data.txt", "data\n");
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { files } from "codemod/files";
export default async function main({ target }) {
  return step("Read", () => {
    let outside;
    try { files.read("../outside.txt"); } catch (error) { outside = error.message.includes("inside the target"); }
    files.create("dir/new.txt", "new\n");
    return {
      data: files.read(`${target}/data.txt`).trim(),
      relative: files.read("data.txt").trim(),
      exists: files.exists("a.txt"),
      listed: files.list("**/*.txt"),
      created: files.read("dir/new.txt").trim(),
      outside,
    };
  });
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "data": "data", "relative": "data", "exists": true, "outside": true,
            "created": "new", "listed": ["a.txt", "data.txt", "dir/new.txt"] })
    );
    assert_eq!(h.read("dir/new.txt"), "new\n");
}

/// Capabilities a selector asks for are not given: the step's log says so,
/// and the callback still cannot write.
#[test]
fn capabilities_a_selector_asks_for_are_not_given() {
    let h = Harness::with_services(
        Arc::new(codemod_workflow::MemoryStore::default()),
        Arc::new(codemod_workflow::interaction::NonInteractive { approve_all: true }),
        codemod_workflow::Services {
            capabilities: [codemod_llrt_capabilities::types::LlrtSupportedModules::Fs].into(),
            ..codemod_workflow::Services::default()
        },
    );
    h.write("a.txt", "a\n");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main({ target }) {
  return step(forEachFile({ include: "a.txt", capabilities: ["fs", "fetch"] }, async (file) => {
    const fs = await import("fs");
    fs.writeFileSync(`${target}/b.txt`, "x");
  }));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("codemod/files"), "{error}");
    assert!(!h.exists("b.txt"));
    let logged = h.events.take().into_iter().any(|event| {
        matches!(event, Event::StepLog { line, .. } if line.contains("capabilities (fs, fetch) are no longer given to callbacks"))
    });
    assert!(logged);

    let error = output(&h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  return step(forEachFile({ include: "a.txt", capabilities: ["disk"] }, () => {}));
}
"#,
        Value::Null,
    ))
    .unwrap_err();
    assert!(error.contains("capabilities must be an array"), "{error}");
}

/// `fetch` and timers written in a workflow file fail its build, in the
/// body and in callbacks, before any run is saved.
#[test]
fn fetch_and_timers_in_a_workflow_file_are_build_errors() {
    let h = Harness::new();
    let error = load_error(
        &h,
        r#"
export default async function main() {
  const response = await fetch("https://example.com");
  return response.status;
}
"#,
    );
    assert!(
        error.contains("fetch is not available in the workflow body"),
        "{error}"
    );
    assert!(error.contains("http(url, options)"), "{error}");

    let error = load_error(
        &h,
        r#"
import { step } from "codemod/workflow";
export default async function main() {
  return step("Wait", () => new Promise((resolve) => setTimeout(resolve, 1)));
}
"#,
    );
    assert!(
        error.contains("setTimeout is not available in workflow callbacks"),
        "{error}"
    );
    assert!(error.contains("sleep("), "{error}");
}

// ---------------------------------------------------------------- http ---

#[test]
fn an_http_step_sends_the_request_and_returns_the_response() {
    let server = server(200);
    let h = Harness::new();
    std::env::set_var("CODEMOD_TEST_HTTP_TOKEN", "s3cret");
    let report = h.start(
        &format!(
            r#"
import {{ step, http }} from "codemod/workflow";
export default async function main() {{
  const got = await step("Get", http("{url}/a?x=1"));
  const posted = await step("Post", http({{
    url: "{url}/b",
    method: "post",
    headers: {{ authorization: {{ env: "CODEMOD_TEST_HTTP_TOKEN" }}, "x-plain": "yes" }},
    json: {{ hello: "world" }},
  }}));
  return {{ status: got.status, ok: got.ok, line: got.json.line, posted: posted.json }};
}}
"#,
            url = server.url
        ),
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({
            "status": 200,
            "ok": true,
            "line": "GET /a?x=1 HTTP/1.1",
            "posted": { "line": "POST /b HTTP/1.1", "body": "{\"hello\":\"world\"}" },
        })
    );
    assert_eq!(server.hits.load(Ordering::SeqCst), 2);
    let requests = server.requests.lock().unwrap();
    assert!(
        requests[1].contains("authorization: s3cret"),
        "{}",
        requests[1]
    );
    assert!(requests[1].contains("x-plain: yes"), "{}", requests[1]);
    // The run records the variable's name, not its value.
    let recorded = serde_json::to_string(&report.steps).unwrap();
    assert!(!recorded.contains("s3cret"), "{recorded}");
}

/// The response is recorded with the step: a resumed run gets it back
/// without sending the request again.
#[test]
fn a_resumed_run_does_not_send_an_http_request_again() {
    let server = server(200);
    let h = Harness::new();
    let source = |fixed: bool| {
        format!(
            r#"
import {{ step, http }} from "codemod/workflow";
export default async function main() {{
  const response = await step("Ask", http("{url}/once"));
  return step("Use", () => {{
    if (!{fixed}) throw new Error("not yet");
    return response.json.line;
  }});
}}
"#,
            url = server.url
        )
    };
    let report = h.start(&source(false), Value::Null);
    assert!(output(&report).unwrap_err().contains("not yet"));
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);

    let resumed = h.resume(&report, Some(&source(true)));
    assert_eq!(output(&resumed).unwrap(), json!("GET /once HTTP/1.1"));
    assert_eq!(
        server.hits.load(Ordering::SeqCst),
        1,
        "the request was sent again"
    );
}

#[test]
fn an_error_status_fails_the_step_unless_it_is_allowed() {
    let server = server(500);
    let h = Harness::new();
    let error = output(&h.start(
        &format!(
            r#"
import {{ step, http }} from "codemod/workflow";
export default async function main() {{
  return step("Fail", http("{url}/x"));
}}
"#,
            url = server.url
        ),
        Value::Null,
    ))
    .unwrap_err();
    assert!(error.contains("answered 500"), "{error}");

    let report = h.start(
        &format!(
            r#"
import {{ step, http }} from "codemod/workflow";
export default async function main() {{
  const {{ status, ok }} = await step("Allowed", http("{url}/x", {{ allowErrorStatus: true }}));
  return {{ status, ok }};
}}
"#,
            url = server.url
        ),
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!({ "status": 500, "ok": false })
    );
}

/// A dry run says what it would request and sends nothing, unless the step
/// asks for `dryRun: "perform"`.
#[test]
fn a_dry_run_skips_http_steps_unless_they_ask_to_perform() {
    let server = server(200);
    let h = Harness::new();
    let report = h.dry_run(&format!(
        r#"
import {{ step, http }} from "codemod/workflow";
export default async function main() {{
  const skipped = await step("Skipped", http("{url}/a", {{ method: "DELETE" }}));
  const performed = await step("Performed", http("{url}/b", {{ dryRun: "perform" }}));
  return {{ skipped, status: performed.status }};
}}
"#,
        url = server.url
    ));
    assert_eq!(
        output(&report).unwrap(),
        json!({ "skipped": { "skipped": true }, "status": 200 })
    );
    assert_eq!(server.hits.load(Ordering::SeqCst), 1);
    let said = h.events.take().into_iter().any(|event| {
        matches!(event, Event::StepLog { line, .. } if line.contains("would request DELETE"))
    });
    assert!(said);
}

#[test]
fn http_options_are_checked_when_the_step_is_described() {
    let h = Harness::new();
    for (call, message) in [
        (
            "http(\"ftp://x\")",
            "url must be an http:// or https:// URL",
        ),
        (
            "http(\"https://x\", { body: \"a\", json: {} })",
            "body (text) or json, not both",
        ),
        (
            "http(\"https://x\", { headers: { a: 1 } })",
            "header 'a' must be a string",
        ),
        ("http(\"https://x\", { dryRun: \"yes\" })", "dryRun must be"),
        (
            "http(\"https://x\", { verb: \"GET\" })",
            "unknown option 'verb'",
        ),
    ] {
        let error = output(&h.start(
            &format!(
                "import {{ step, http }} from \"codemod/workflow\";\nexport default async function main() {{ return step({call}); }}\n"
            ),
            Value::Null,
        ))
        .unwrap_err();
        assert!(error.contains(message), "{call}: {error}");
    }
}

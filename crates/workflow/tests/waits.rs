//! Retries and timeouts on steps, `sleep` and `waitForEvent`.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use codemod_workflow::events::Event;
use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::{
    send_event, Ending, EngineError, MemoryStore, RunStatus, RunStore, StepStatus,
};
use common::{output, Harness};
use serde_json::{json, Value};

fn harness() -> (Harness, Arc<dyn RunStore>) {
    let store: Arc<dyn RunStore> = Arc::new(MemoryStore::default());
    let harness = Harness::with(
        Arc::clone(&store),
        Arc::new(NonInteractive { approve_all: true }),
    );
    (harness, store)
}

/// Run `then` with the id of the run the harness starts next, once it exists.
fn when_started(
    store: &Arc<dyn RunStore>,
    then: impl FnOnce(&dyn RunStore, &str) + Send + 'static,
) {
    let store = Arc::clone(store);
    std::thread::spawn(move || {
        for _ in 0..500 {
            if let Some(run) = store.list_runs(1).unwrap().into_iter().next() {
                return then(store.as_ref(), &run.id);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
}

// ------------------------------------------------------------ retries ----

/// Fails until it has been tried three times.
const THIRD_TIME_LUCKY: &str = r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  await step(
    "Flaky",
    shell(`echo x >> ${target}/tries.txt; test $(wc -l < ${target}/tries.txt) -ge 3`),
    { retries: RETRIES, retryDelay: 0 },
  );
  return "done";
}
"#;

#[test]
fn a_failed_step_runs_again_while_it_has_retries() {
    let (h, _) = harness();
    let report = h.start(&THIRD_TIME_LUCKY.replace("RETRIES", "2"), Value::Null);

    assert_eq!(output(&report).unwrap(), json!("done"));
    assert_eq!(h.read("tries.txt"), "x\nx\nx\n");
    assert_eq!(report.steps.len(), 1);
    assert_eq!(report.steps[0].attempts, Some(3));
    let retries: Vec<_> = h
        .events
        .take()
        .into_iter()
        .filter_map(|event| match event {
            Event::StepRetrying {
                attempt, attempts, ..
            } => Some((attempt, attempts)),
            _ => None,
        })
        .collect();
    assert_eq!(retries, [(1, 3), (2, 3)]);
}

#[test]
fn a_step_out_of_retries_fails_with_its_last_error() {
    let (h, _) = harness();
    let report = h.start(&THIRD_TIME_LUCKY.replace("RETRIES", "1"), Value::Null);

    let error = output(&report).unwrap_err();
    assert!(error.contains("step 'Flaky' failed"), "{error}");
    assert_eq!(h.read("tries.txt"), "x\nx\n");
    assert_eq!(report.steps[0].status, StepStatus::Failed);
    assert_eq!(report.steps[0].attempts, Some(2));

    // A resume gives the step its attempts again.
    let resumed = h.resume(&report, None);
    assert_eq!(output(&resumed).unwrap(), json!("done"));
    assert_eq!(h.read("tries.txt"), "x\nx\nx\n");
}

#[test]
fn how_a_step_is_run_does_not_make_it_another_step() {
    let (h, _) = harness();
    let source = r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  await step("Once", shell(`echo x >> ${target}/log.txt`), OPTIONS);
  await step("Gate", shell(`test -f ${target}/go`));
  return "done";
}
"#;
    let first = h.start(&source.replace("OPTIONS", "{ retries: 1 }"), Value::Null);
    assert!(output(&first).is_err());

    h.write("go", "");
    let changed = source.replace("OPTIONS", "{ retries: 5, timeout: \"1m\" }");
    let second = h.resume(&first, Some(&changed));
    assert_eq!(output(&second).unwrap(), json!("done"));
    assert_eq!(
        h.read("log.txt"),
        "x\n",
        "the finished step did not run again"
    );
}

#[test]
fn step_options_are_checked() {
    let (h, _) = harness();
    for (options, expected) in [
        ("{ retry: 2 }", "unknown option 'retry'"),
        ("{ retries: -1 }", "retries must be a whole number"),
        (
            "{ timeout: \"soon\" }",
            "timeout: expected milliseconds or a duration",
        ),
        ("{ timeout: 0 }", "timeout must be more than 0"),
    ] {
        let report = h.start(
            &format!(
                r#"
import {{ step, shell }} from "codemod/workflow";
export default async function main() {{
  await step("Checked", shell("true"), {options});
}}
"#
            ),
            Value::Null,
        );
        let error = output(&report).unwrap_err();
        assert!(error.contains(expected), "{options}: {error}");
        assert!(error.contains("step 'Checked'"), "{error}");
    }
}

// ----------------------------------------------------------- timeouts ----

#[test]
fn a_command_that_takes_too_long_is_stopped_and_the_run_goes_on() {
    let (h, _) = harness();
    let started = Instant::now();
    let report = h.start(
        r#"
import { step, shell } from "codemod/workflow";
export default async function main({ target }) {
  let error = null;
  try {
    await step("Slow", shell(`sleep 30; echo late > ${target}/late.txt`), { timeout: "300ms" });
  } catch (caught) {
    error = caught.message;
  }
  await step("After", shell(`echo after > ${target}/after.txt`));
  return error;
}
"#,
        Value::Null,
    );

    assert_eq!(
        output(&report).unwrap(),
        json!("step 'Slow' failed: timed out after 300ms")
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(report.run.status, RunStatus::Succeeded);
    assert_eq!(report.steps[0].status, StepStatus::Failed);
    assert!(h.exists("after.txt"));
    assert!(!h.exists("late.txt"));
}

#[test]
fn a_function_step_that_never_returns_is_stopped() {
    let (h, _) = harness();
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
export default async function main() {
  await step("Spin", () => { for (;;) {} }, { timeout: 300 });
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("timed out after 300ms"), "{error}");
}

#[test]
fn a_timed_out_step_writes_none_of_its_file_changes() {
    let (h, _) = harness();
    h.write("a.txt", "a");
    h.write("b.txt", "b");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
export default async function main() {
  await step(
    "Edit",
    forEachFile("*.txt", (file) => {
      file.write(file.text + "+");
      if (file.path === "b.txt") for (;;) {}
    }),
    { timeout: 400 },
  );
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("timed out after 400ms"), "{error}");
    assert_eq!(h.read("a.txt"), "a");
    assert_eq!(h.read("b.txt"), "b");
}

// -------------------------------------------------------------- sleep ----

#[test]
fn sleep_waits_and_is_a_step() {
    let (h, _) = harness();
    let started = Instant::now();
    let report = h.start(
        r#"
import { sleep } from "codemod/workflow";
export default async function main() {
  return [await sleep("200ms"), await sleep(0)];
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!([null, null]));
    assert!(started.elapsed() >= Duration::from_millis(200));
    let labels: Vec<_> = report.steps.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["sleep 200ms", "sleep 0ms"]);
    assert!(report
        .steps
        .iter()
        .all(|s| s.status == StepStatus::Succeeded));
    // Beginning to wait is saved, not reported as the step finishing.
    let finished = h
        .events
        .take()
        .into_iter()
        .filter(|event| matches!(event, Event::StepFinished { .. }))
        .count();
    assert_eq!(finished, 2);
}

#[test]
fn an_interrupted_sleep_goes_on_from_when_it_began() {
    let (h, store) = harness();
    when_started(&store, |store, run_id| {
        std::thread::sleep(Duration::from_millis(200));
        store.request_cancel(run_id).unwrap();
    });
    let first = h.start(
        r#"
import { sleep } from "codemod/workflow";
export default async function main() {
  await sleep("1h");
  return "rested";
}
"#,
        Value::Null,
    );
    assert_eq!(first.ending, Ending::Canceled);
    let mut step = first.steps[0].clone();
    assert_eq!(step.status, StepStatus::Canceled);
    let since = step.since.expect("the wait's beginning is kept");

    // As if the hour had passed while the run was stopped.
    step.since = Some(since - 3_600_000);
    store.put_step(&first.run.id, &step).unwrap();
    let started = Instant::now();
    let second = h.resume(&first, None);
    assert_eq!(output(&second).unwrap(), json!("rested"));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(second.steps[0].since, Some(since - 3_600_000));
}

#[test]
fn a_dry_run_does_not_sleep_and_cannot_wait_for_events() {
    let (h, _) = harness();
    let started = Instant::now();
    let report = h.dry_run(
        r#"
import { sleep, waitForEvent } from "codemod/workflow";
export default async function main() {
  await sleep("1h");
  await waitForEvent("go");
}
"#,
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    let error = output(&report).unwrap_err();
    assert!(error.contains("a dry run is not saved"), "{error}");
}

// ------------------------------------------------------------- events ----

const WAIT_FOR_GO: &str = r#"
import { waitForEvent } from "codemod/workflow";
export default async function main() {
  const event = await waitForEvent("go"OPTIONS);
  return { event };
}
"#;

#[test]
fn a_waiting_step_takes_the_event_sent_to_its_run() {
    let (h, store) = harness();
    when_started(&store, |store, run_id| {
        // Not this one.
        send_event(store, run_id, "stop", &Value::Null).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        send_event(store, run_id, "go", &json!({ "passed": true })).unwrap();
    });
    let report = h.start(&WAIT_FOR_GO.replace("OPTIONS", ""), Value::Null);

    assert_eq!(
        output(&report).unwrap(),
        json!({ "event": { "passed": true } })
    );
    assert_eq!(report.steps[0].label, "wait for go");
    assert_eq!(report.steps[0].status, StepStatus::Succeeded);
}

#[test]
fn a_wait_that_times_out_fails_and_a_resume_waits_again() {
    let (h, store) = harness();
    let source = WAIT_FOR_GO.replace("OPTIONS", ", { timeout: \"300ms\" }");
    let first = h.start(&source, Value::Null);
    let error = output(&first).unwrap_err();
    assert!(error.contains("no 'go' event came within 300ms"), "{error}");

    // Sent while nothing runs: kept for the step.
    send_event(store.as_ref(), &first.run.id, "go", &json!(1)).unwrap();
    let second = h.resume(&first, None);
    assert_eq!(output(&second).unwrap(), json!({ "event": 1 }));
}

#[test]
fn events_are_taken_once_each_in_the_order_sent() {
    let (h, store) = harness();
    when_started(&store, |store, run_id| {
        send_event(store, run_id, "item", &json!("first")).unwrap();
        send_event(store, run_id, "item", &json!("second")).unwrap();
    });
    let report = h.start(
        r#"
import { waitForEvent } from "codemod/workflow";
export default async function main() {
  const one = await waitForEvent("item", { timeout: "5s" });
  const two = await waitForEvent("item", { timeout: "5s" });
  return [one, two];
}
"#,
        Value::Null,
    );
    assert_eq!(output(&report).unwrap(), json!(["first", "second"]));
}

#[test]
fn sending_checks_the_name_and_the_run() {
    let (h, store) = harness();
    let report = h.start(
        r#"
export default async function main() {
  return "done";
}
"#,
        Value::Null,
    );
    let send = |run: &str, name: &str| send_event(store.as_ref(), run, name, &Value::Null);
    assert!(matches!(
        send(&report.run.id, "not a name"),
        Err(EngineError::EventName(_))
    ));
    assert!(matches!(
        send("missing", "go"),
        Err(EngineError::NotFound(_))
    ));
    assert!(matches!(
        send(&report.run.id, "go"),
        Err(EngineError::AlreadyFinished(_))
    ));
}

#[test]
fn a_bad_event_name_or_duration_is_refused_where_it_is_written() {
    let (h, _) = harness();
    for (call, expected) in [
        ("waitForEvent(\"two words\")", "the event name must be"),
        (
            "waitForEvent(\"go\", { within: 1 })",
            "unknown option 'within'",
        ),
        ("sleep(\"a while\")", "sleep: expected milliseconds"),
    ] {
        let report = h.start(
            &format!(
                r#"
import {{ sleep, waitForEvent }} from "codemod/workflow";
export default async function main() {{
  await {call};
}}
"#
            ),
            Value::Null,
        );
        let error = output(&report).unwrap_err();
        assert!(error.contains(expected), "{call}: {error}");
    }
}

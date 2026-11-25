//! Model toolkit, end to end: two models that live outside the engine
//! (`examples/models/next-app-routes`, `examples/models/http-client`) run
//! over a frontend and a Next.js backend, and their facts show up in
//! `callers()`, `callees()`, `flowsTo()` and `flowsFrom()`.
//!
//! The questions are asked by `examples/models/e2e/two-apps.ts`, which
//! answers in JSON.

use ast_grep_language::SupportLang;
use codemod_sandbox::sandbox::engine::execution_engine::{
    execute_codemod_with_quickjs, CodemodOutput, ExecutionResult, JssgExecutionOptions,
};
use codemod_sandbox::sandbox::resolvers::oxc_resolver::OxcResolver;
use codemod_sandbox::CodemodLang;
use language_core::SemanticProvider;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const ROUTE: &str = "api/app/api/users/[id]/route.ts";
const PAGE: &str = "web/src/users/UserLookup.tsx";

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/models")
        .canonicalize()
        .expect("examples/models")
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Run a script on a file of a fixture and return its JSON answer.
///
/// Model facts belong to a workspace (every provider of a root shares
/// them), so each run gets its own copy of the fixture.
async fn run(
    fixture: &str,
    script: &str,
    target: &str,
    language: SupportLang,
    models: &str,
) -> Value {
    let examples = examples();
    let temp = tempfile::TempDir::new().unwrap();
    copy_dir(&examples.join("fixtures").join(fixture), temp.path());
    let workspace = temp.path().canonicalize().unwrap();
    let script = examples.join("e2e").join(script);
    let target = workspace.join(target);
    let content = std::fs::read_to_string(&target).expect("the target file");
    // The provider the CLI uses: one per language, made on first use.
    let provider: Arc<dyn SemanticProvider> = Arc::new(
        semantic_factory::LazySemanticProvider::workspace_scope(workspace.clone()),
    );
    let options = JssgExecutionOptions {
        script_path: &script,
        resolver: Arc::new(OxcResolver::new(examples.join("e2e"), None).unwrap()),
        language: CodemodLang::Static(language),
        file_path: &target,
        content: &content,
        selector_config: None,
        params: Some(HashMap::from([(
            "models".to_string(),
            Value::String(models.to_string()),
        )])),
        matrix_values: None,
        capabilities: None,
        semantic_provider: Some(provider),
        llm_request_handler: None,
        runtime_event_callback: None,
        cancellation_flag: None,
        test_mode: false,
        dry_run: true,
        target_directory: &workspace,
        pending_files: None,
    };
    match execute_codemod_with_quickjs(options).await {
        Ok(CodemodOutput {
            primary: ExecutionResult::Modified(modified),
            ..
        }) => serde_json::from_str(&modified.content).expect("the script answers in JSON"),
        other => panic!("the script did not answer: {other:?}"),
    }
}

/// The two-app questions, with the models or without.
async fn ask(models: &str) -> Value {
    run(
        "two-apps",
        "two-apps.ts",
        ROUTE,
        SupportLang::TypeScript,
        models,
    )
    .await
}

fn list<'a>(answers: &'a Value, name: &str) -> &'a Vec<Value> {
    answers[name]
        .as_array()
        .unwrap_or_else(|| panic!("{name} is missing"))
}

/// One line per item, for the test's output.
fn show(title: &str, items: &[Value], fields: &[&str]) {
    println!("\n{title}");
    if items.is_empty() {
        println!("  (none)");
    }
    for item in items {
        let line: Vec<String> = fields
            .iter()
            .filter(|field| !item[**field].is_null())
            .map(|field| match &item[*field] {
                Value::String(text) => format!("{field}={text}"),
                other => format!("{field}={other}"),
            })
            .collect();
        println!("  {}", line.join("  "));
    }
}

const CALLER: &[&str] = &["depth", "via", "name", "file", "site", "models", "channel"];
const STEP: &[&str] = &["depth", "via", "kind", "file", "text", "models", "tags"];
const STEP_TEXT: &[&str] = &["depth", "via", "file", "text", "models", "tags"];

#[tokio::test]
async fn models_join_a_frontend_to_its_next_backend() {
    let answers = ask("on").await;
    println!("models applied: {}", answers["applied"]);

    // callers() of the route handler: Next (the entry), and the frontend
    // call that requests this route, through the channel.
    let callers = list(&answers, "handlerCallers");
    show(
        "callers() of GET in api/app/api/users/[id]/route.ts",
        callers,
        CALLER,
    );
    let entry = callers
        .iter()
        .find(|c| c["via"] == "entry-point")
        .expect("the handler is an entry point");
    assert_eq!(entry["models"], serde_json::json!(["next-app-routes"]));
    assert_eq!(entry["channel"], "GET /api/users/[id]");
    let request = callers
        .iter()
        .find(|c| c["via"] == "channel")
        .expect("the frontend's request calls the handler");
    assert_eq!(request["name"], "onSubmit");
    assert_eq!(request["file"], PAGE);
    assert_eq!(request["site"], "fetch(`/api/users/${id}`)");
    assert_eq!(
        request["models"],
        serde_json::json!(["http-client", "next-app-routes"])
    );
    assert_eq!(request["channel"], "http GET /api/users/{id}");
    // `models: false` on the query leaves them out.
    let bare = list(&answers, "handlerCallersWithoutModels");
    show(
        "callers({ models: false }) of the same handler",
        bare,
        CALLER,
    );
    assert!(bare.is_empty());

    // Transitive: the database function is reached from the frontend.
    let callers = list(&answers, "findUserCallers");
    show(
        "callers({ transitive: true }) of findUser in api/lib/db.ts",
        callers,
        CALLER,
    );
    assert!(callers
        .iter()
        .any(|c| c["via"] == "direct" && c["name"] == "GET" && c["depth"] == 1));
    assert!(callers
        .iter()
        .any(|c| c["via"] == "channel" && c["name"] == "onSubmit" && c["depth"] == 2));

    // flowsTo() from what the user typed: across the channel into the
    // handler's `params`, and on to the database call.
    let steps = list(&answers, "typedFlowsTo");
    show(
        "flowsTo() from `form.elements.userId.value` in the frontend",
        steps,
        STEP,
    );
    let crossing = steps
        .iter()
        .find(|s| s["via"] == "channel")
        .expect("the value crosses the channel");
    assert_eq!(crossing["file"], ROUTE);
    assert_eq!(crossing["text"], "params");
    assert_eq!(
        crossing["models"],
        serde_json::json!(["http-client", "next-app-routes"])
    );
    assert!(crossing["tags"]
        .as_array()
        .unwrap()
        .contains(&Value::String("source:user-input".to_string())));
    assert!(steps
        .iter()
        .any(|s| s["file"] == ROUTE && s["via"] == "argument" && s["text"] == "findUser(id)"));
    let bare = list(&answers, "typedFlowsToWithoutModels");
    assert!(!bare.is_empty());
    assert!(bare.iter().all(|s| s["file"] == PAGE));

    // callees() of the submit handler includes the route handler.
    let callees = list(&answers, "onSubmitCallees");
    show(
        "callees() of onSubmit (resolved)",
        callees,
        &["via", "name", "file", "models"],
    );
    assert!(callees
        .iter()
        .any(|c| c["via"] == "channel" && c["name"] == "GET" && c["file"] == ROUTE));

    // flowsFrom(): what the page shows comes from the handler's response.
    let steps = list(&answers, "shownFlowsFrom");
    show("flowsFrom() of `user` in showUser(user)", steps, STEP);
    assert!(steps
        .iter()
        .any(|s| s["via"] == "channel" && s["file"] == ROUTE && s["text"] == "user"));

    // Every end of a channel, joined or not.
    let channels = list(&answers, "channels");
    show(
        "channels",
        channels,
        &["side", "key", "file", "matches", "open"],
    );
    let end = |key: &str, file: &str| {
        channels
            .iter()
            .find(|c| c["key"] == key && c["file"].as_str().unwrap().ends_with(file))
            .unwrap_or_else(|| panic!("no channel {key} in {file}"))
    };
    // A request nothing answers, a route nothing requests, a URL the model
    // could not read.
    assert_eq!(end("http GET /api/teams", "actions.ts")["matches"], 0);
    assert_eq!(end("http GET /api/health", "health/route.ts")["matches"], 0);
    assert_eq!(end("http GET /?", "client.ts")["open"], true);
    // axios with a baseURL, a constant, a concatenation.
    assert_eq!(end("http GET /api/users", "actions.ts")["matches"], 1);
    assert_eq!(
        end("http PATCH /api/users/{p0}", "actions.ts")["matches"],
        1
    );
    assert_eq!(end("http POST /api/users", "actions.ts")["matches"], 1);
    assert_eq!(
        end("http DELETE /api/users/{p0}", "actions.ts")["matches"],
        1
    );

    // The model's own test: its facts for one file, against the text kept
    // next to the model.
    let facts = answers["facts"].as_str().unwrap();
    println!("\nfacts of next-app-routes for {ROUTE}\n{facts}");
    let expected_path = examples().join("next-app-routes/tests/users-id.facts.txt");
    if std::env::var("UPDATE_MODEL_FACTS").is_ok() {
        std::fs::create_dir_all(expected_path.parent().unwrap()).unwrap();
        std::fs::write(&expected_path, format!("{facts}\n")).unwrap();
    }
    let expected = std::fs::read_to_string(&expected_path).unwrap_or_default();
    assert_eq!(facts.trim(), expected.trim());

    if let Ok(out) = std::env::var("MODEL_MANIFEST_OUT") {
        std::fs::write(
            out,
            serde_json::to_string_pretty(&answers["manifest"]).unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test]
async fn without_models_the_two_apps_are_not_joined() {
    let answers = ask("off").await;
    let callers = list(&answers, "handlerCallers");
    show("no models: callers() of GET", callers, CALLER);
    assert!(callers.is_empty());

    let callers = list(&answers, "findUserCallers");
    show(
        "no models: callers({ transitive: true }) of findUser",
        callers,
        CALLER,
    );
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0]["name"], "GET");

    let steps = list(&answers, "typedFlowsTo");
    show("no models: flowsTo() from the typed value", steps, STEP);
    assert!(steps.iter().all(|s| s["file"] == PAGE));
    assert!(list(&answers, "onSubmitCallees")
        .iter()
        .all(|c| c["via"] != "channel"));
    assert!(list(&answers, "shownFlowsFrom")
        .iter()
        .all(|s| s["file"] == PAGE));
    assert!(answers["handlerTags"].as_array().unwrap().is_empty());
}

const VIEWS: &str = "api/controllers/apps.py";
const BUTTON: &str = "web/app/apps/[id]/run-button.tsx";

/// A TypeScript frontend, a Flask API and Celery tasks: the same models
/// API, facts about files of two languages, channels joined across them.
#[tokio::test]
async fn models_join_typescript_to_python() {
    let answers = run(
        "dify-like",
        "dify-like.ts",
        VIEWS,
        SupportLang::Python,
        "on",
    )
    .await;
    println!("models applied: {}", answers["applied"]);
    let both = serde_json::json!(["http-client", "flask-routes"]);

    // Python asked: the Flask view is called by Flask, and by the fetch in
    // the TypeScript frontend.
    let callers = list(&answers, "viewCallers");
    show(
        "callers() of run_app in api/controllers/apps.py (Python)",
        callers,
        CALLER,
    );
    let entry = callers.iter().find(|c| c["via"] == "entry-point").unwrap();
    assert_eq!(entry["models"], serde_json::json!(["flask-routes"]));
    assert_eq!(entry["channel"], "POST /console/api/apps/<uuid:app_id>/run");
    let request = callers.iter().find(|c| c["via"] == "channel").unwrap();
    assert_eq!(request["name"], "run");
    assert_eq!(request["file"], BUTTON);
    assert_eq!(request["models"], both);

    // … and it calls the Celery task, through `.delay()`.
    let callees = list(&answers, "viewCallees");
    show(
        "callees() of run_app (Python)",
        callees,
        &["via", "name", "file", "models", "channel"],
    );
    assert!(callees.iter().any(|c| c["via"] == "channel"
        && c["name"] == "run_app_task"
        && c["file"] == "api/tasks/run_app.py"
        && c["models"] == serde_json::json!(["celery-tasks"])));

    // The task, transitively: the worker, the view, and the frontend.
    let callers = list(&answers, "taskCallers");
    show(
        "callers({ transitive: true }) of run_app_task (Python)",
        callers,
        CALLER,
    );
    assert!(callers
        .iter()
        .any(|c| c["via"] == "entry-point" && c["depth"] == 1));
    assert!(callers
        .iter()
        .any(|c| c["via"] == "channel" && c["name"] == "run_app" && c["depth"] == 1));
    assert!(callers.iter().any(|c| c["via"] == "channel"
        && c["name"] == "run"
        && c["file"] == BUTTON
        && c["depth"] == 2));

    // A task sent by name; a task nothing sends.
    let callers = list(&answers, "mailCallers");
    show("callers() of send_invite (sent by name)", callers, CALLER);
    assert!(callers.iter().any(|c| c["via"] == "channel"
        && c["name"] == "invite_member"
        && c["file"] == "api/services/invite.py"));
    let callers = list(&answers, "digestCallers");
    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0]["via"], "entry-point");

    // TypeScript asked: the frontend's `run` calls the Python view.
    let callees = list(&answers, "runCallees");
    show(
        "callees() of run in the frontend (TypeScript)",
        callees,
        &["via", "name", "file", "models", "channel"],
    );
    assert!(callees.iter().any(|c| c["via"] == "channel"
        && c["name"] == "run_app"
        && c["file"] == VIEWS
        && c["models"] == both));

    // TypeScript → Python: the app id in the URL lands in the view's
    // parameter; the body in `request.get_json()`.
    let steps = list(&answers, "appIdFlowsTo");
    show("flowsTo() from `appId` in the frontend", steps, STEP_TEXT);
    let landed = steps.iter().find(|s| s["via"] == "channel").unwrap();
    assert_eq!(landed["file"], VIEWS);
    assert_eq!(landed["text"], "app_id");
    assert!(landed["tags"]
        .as_array()
        .unwrap()
        .contains(&Value::String("source:user-input".to_string())));
    let steps = list(&answers, "bodyFlowsTo");
    show(
        "flowsTo() from the request body in the frontend",
        steps,
        STEP_TEXT,
    );
    assert!(steps
        .iter()
        .any(|s| s["via"] == "channel" && s["file"] == VIEWS && s["text"] == "request.get_json()"));

    // Python → Python across the queue: the task's parameter is the view's argument.
    let steps = list(&answers, "taskParameterFlowsFrom");
    show(
        "flowsFrom() of the task's `app_id` parameter (Python)",
        steps,
        STEP_TEXT,
    );
    assert!(steps
        .iter()
        .any(|s| s["via"] == "channel" && s["file"] == VIEWS && s["text"] == "app_id"));

    let channels = list(&answers, "channels");
    show(
        "channels",
        channels,
        &["side", "key", "file", "matches", "open"],
    );
    let end = |key: &str, file: &str| {
        channels
            .iter()
            .find(|c| c["key"] == key && c["file"].as_str().unwrap().ends_with(file))
            .unwrap_or_else(|| panic!("no channel {key} in {file}"))
    };
    assert_eq!(
        end("http POST /console/api/apps/{p0}/run", "run-button.tsx")["matches"],
        1
    );
    assert_eq!(
        end("http POST /console/api/apps/{p0}/invite", "invite.ts")["matches"],
        1
    );
    assert_eq!(
        end("http GET /console/api/apps/{app_id}", "apps.py")["matches"],
        0
    );
    assert_eq!(end("celery mail.send_invite", "invite.py")["matches"], 1);
}

#[tokio::test]
async fn without_models_python_has_no_answers() {
    let answers = run(
        "dify-like",
        "dify-like.ts",
        VIEWS,
        SupportLang::Python,
        "off",
    )
    .await;
    for name in [
        "viewCallers",
        "viewCallees",
        "taskCallers",
        "mailCallers",
        "runCallees",
    ] {
        assert!(list(&answers, name).is_empty(), "{name}: {}", answers[name]);
    }
    assert!(list(&answers, "appIdFlowsTo")
        .iter()
        .all(|s| s["file"] == BUTTON));
}

use anyhow::Result;
use butterflow_core::ai_handoff::{discover_installed_agents, AgentOption};
use butterflow_core::report::{ExecutionReport, ShareLevel};
use hyper::body::to_bytes;
use hyper::header::{CONTENT_TYPE, HOST, ORIGIN};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, HeaderMap, Method, Request, Response, Server, StatusCode};
use std::convert::Infallible;
use std::net::TcpListener;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::time::{sleep, timeout};

use crate::auth::{OidcClient, TokenStorage};
use crate::feedback;

const FEEDBACK_CATEGORY: &str = "codemod-performance";
const FEEDBACK_AGENT_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_FEEDBACK_MESSAGE_LEN: usize = 4000;
const REPORT_SERVER_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const REPORT_SERVER_IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

struct ReportServerState {
    last_activity: Mutex<Instant>,
    active_feedback_jobs: AtomicUsize,
}

impl ReportServerState {
    fn new() -> Self {
        Self {
            last_activity: Mutex::new(Instant::now()),
            active_feedback_jobs: AtomicUsize::new(0),
        }
    }

    fn touch(&self) {
        if let Ok(mut last_activity) = self.last_activity.lock() {
            *last_activity = Instant::now();
        }
    }

    fn idle_for(&self) -> Duration {
        self.last_activity
            .lock()
            .map(|last_activity| last_activity.elapsed())
            .unwrap_or_default()
    }

    fn has_active_feedback_jobs(&self) -> bool {
        self.active_feedback_jobs.load(Ordering::Relaxed) > 0
    }

    fn feedback_job_guard(self: &Arc<Self>) -> FeedbackJobGuard {
        self.active_feedback_jobs.fetch_add(1, Ordering::Relaxed);
        self.touch();
        FeedbackJobGuard {
            state: Arc::clone(self),
        }
    }
}

struct FeedbackJobGuard {
    state: Arc<ReportServerState>,
}

impl Drop for FeedbackJobGuard {
    fn drop(&mut self) {
        self.state
            .active_feedback_jobs
            .fetch_sub(1, Ordering::Relaxed);
        self.state.touch();
    }
}

/// Embedded report HTML (built by Preact SPA in report-ui/)
#[cfg(not(debug_assertions))]
const REPORT_HTML: &str = include_str!("../report-ui/dist/index.html");

/// In debug builds, try to read the built SPA from disk at runtime
#[cfg(debug_assertions)]
fn get_report_html() -> String {
    let compile_time_manifest_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("report-ui/dist/index.html");
    let candidates = [
        compile_time_manifest_dir,
        std::path::PathBuf::from("crates/cli/report-ui/dist/index.html"),
        std::path::PathBuf::from("report-ui/dist/index.html"),
    ];

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let path = std::path::PathBuf::from(&manifest_dir).join("report-ui/dist/index.html");
        if let Ok(contents) = std::fs::read_to_string(&path) {
            return contents;
        }
    }

    for candidate in &candidates {
        if let Ok(contents) = std::fs::read_to_string(candidate) {
            return contents;
        }
    }

    r#"<!DOCTYPE html>
<html>
<head><title>Codemod Report</title></head>
<body style="background:#0a0a0a;color:#e5e5e5;font-family:monospace;padding:2rem">
<h1>Report UI not built</h1>
<p>Run <code>cd crates/cli/report-ui && pnpm install && pnpm build</code> to build the report UI.</p>
<p>In the meantime, the raw report JSON is available at <a href="/api/report" style="color:#4ade80">/api/report</a>.</p>
</body>
</html>"#
        .to_string()
}

/// Find an available port in the given range
fn find_available_port(start: u16, end: u16) -> Option<u16> {
    (start..end).find(|&port| TcpListener::bind(("127.0.0.1", port)).is_ok())
}

/// Handle incoming HTTP requests
async fn handle_request(
    req: Request<Body>,
    report: Arc<ExecutionReport>,
    state: Arc<ReportServerState>,
) -> std::result::Result<Response<Body>, Infallible> {
    state.touch();
    let response = match (req.method(), req.uri().path()) {
        (&Method::GET, "/") => {
            #[cfg(not(debug_assertions))]
            let html = REPORT_HTML.to_string();
            #[cfg(debug_assertions)]
            let html = get_report_html();

            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/html; charset=utf-8")
                .body(Body::from(html))
                .unwrap()
        }

        (&Method::GET, "/api/report") => {
            let json = serde_json::to_string(&*report).unwrap_or_default();
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Body::from(json))
                .unwrap()
        }

        (&Method::GET, "/api/auth-status") => match handle_auth_status() {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "error": e.to_string() }).to_string(),
                ))
                .unwrap(),
        },

        (&Method::POST, "/api/share") => match handle_share(req, &report).await {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
            Err(e) => {
                let is_auth_error = e.downcast_ref::<AuthError>().is_some();
                let status = if is_auth_error {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::INTERNAL_SERVER_ERROR
                };
                Response::builder()
                    .status(status)
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "error": e.to_string() }).to_string(),
                    ))
                    .unwrap()
            }
        },

        (&Method::GET, "/api/feedback/status") => match handle_feedback_status() {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "error": e.to_string() }).to_string(),
                ))
                .unwrap(),
        },

        (&Method::POST, "/api/feedback") => match handle_feedback_submit(req).await {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
            Err(e) => Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "error": e.to_string() }).to_string(),
                ))
                .unwrap(),
        },

        (&Method::POST, "/api/feedback/agent/stream") => {
            match handle_feedback_agent_stream(req, report.clone(), state.clone()).await {
                Ok(response) => response,
                Err(e) => Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "error": e.to_string() }).to_string(),
                    ))
                    .unwrap(),
            }
        }

        (&Method::POST, "/api/feedback/agent") => {
            match handle_feedback_agent_submit(req, &report, state.clone()).await {
                Ok(body) => Response::builder()
                    .status(StatusCode::OK)
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
                Err(e) => Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "error": e.to_string() }).to_string(),
                    ))
                    .unwrap(),
            }
        }

        (&Method::POST, "/api/login") => match handle_login().await {
            Ok(body) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "error": e.to_string() }).to_string(),
                ))
                .unwrap(),
        },

        (&Method::POST, "/api/shutdown") => {
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                std::process::exit(0);
            });
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from("{}"))
                .unwrap()
        }

        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("Not Found"))
            .unwrap(),
    };

    Ok(response)
}

/// Body sent by the SPA when sharing
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareRequest {
    #[serde(default = "default_share_level")]
    level: ShareLevel,
}

/// Body sent by the SPA when submitting manual feedback
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackSubmitRequest {
    #[serde(default = "default_feedback_category")]
    category: String,
    message: String,
}

/// Body sent by the SPA when asking an installed agent to submit feedback
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackAgentRequest {
    agent: Option<String>,
}

fn default_share_level() -> ShareLevel {
    ShareLevel::WithFiles
}

fn default_feedback_category() -> String {
    FEEDBACK_CATEGORY.to_string()
}

/// Typed error for authentication failures so the handler can return 401
#[derive(Debug)]
struct AuthError(String);

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AuthError {}

/// Handle auth status checks for the report UI
fn handle_auth_status() -> Result<String> {
    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;
    let registry_url = &config.default_registry;
    let auth = storage.get_auth_for_registry(registry_url)?;

    Ok(serde_json::json!({
        "authenticated": auth.is_some(),
        "username": auth.map(|auth| auth.user.username),
    })
    .to_string())
}

/// Handle the share/upload proxy request
async fn handle_share(req: Request<Body>, report: &ExecutionReport) -> Result<String> {
    // Parse share level from request body
    let body_bytes = to_bytes(req.into_body()).await?;
    let share_req: ShareRequest = if body_bytes.is_empty() {
        ShareRequest {
            level: default_share_level(),
        }
    } else {
        serde_json::from_slice(&body_bytes).unwrap_or(ShareRequest {
            level: default_share_level(),
        })
    };

    let stripped = report.strip_for_sharing(&share_req.level);

    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;
    let registry_url = &config.default_registry;

    let auth = storage
        .get_auth_for_registry(registry_url)?
        .ok_or_else(|| {
            anyhow::anyhow!(AuthError(
                "Not logged in. Run `codemod login` to authenticate.".to_string()
            ))
        })?;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{registry_url}/api/v1/reports"))
        .bearer_auth(&auth.tokens.access_token)
        .json(&stripped)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(anyhow::anyhow!(AuthError(
                "Authentication expired or invalid. Please log in again.".to_string()
            )));
        }

        return Err(anyhow::anyhow!(
            "Failed to upload report: {} {}",
            status,
            body
        ));
    }

    let body = resp.text().await?;
    Ok(body)
}

/// Handle feedback status checks for the report UI
fn handle_feedback_status() -> Result<String> {
    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;
    let discovered_agents = discover_installed_agents();
    let selected_agent = select_feedback_agent(&discovered_agents, None).map(|agent| {
        serde_json::json!({
            "canonical": agent.canonical,
            "label": agent.label,
            "available": agent.is_available(),
        })
    });
    let agents = discovered_agents
        .into_iter()
        .map(|agent| {
            serde_json::json!({
                "canonical": agent.canonical,
                "label": agent.label,
                "available": agent.is_available(),
            })
        })
        .collect::<Vec<_>>();

    Ok(serde_json::json!({
        "disabled": feedback::feedback_disabled(),
        "enabled": config.anonymous_feedback.enabled,
        "consentedAt": config.anonymous_feedback.consented_at,
        "agents": agents,
        "selectedAgent": selected_agent,
    })
    .to_string())
}

/// Handle manual feedback submission from the report UI
async fn handle_feedback_submit(req: Request<Body>) -> Result<String> {
    validate_feedback_request_headers(req.headers())?;

    let body_bytes = to_bytes(req.into_body()).await?;
    if body_bytes.is_empty() {
        anyhow::bail!("Feedback requests must include a JSON body.");
    }

    let submit_req: FeedbackSubmitRequest = serde_json::from_slice(&body_bytes)
        .map_err(|error| anyhow::anyhow!("Invalid feedback request JSON: {error}"))?;
    submit_report_feedback(submit_req.category, submit_req.message).await
}

/// Handle agent-drafted feedback submission from the report UI
async fn handle_feedback_agent_submit(
    req: Request<Body>,
    report: &ExecutionReport,
    state: Arc<ReportServerState>,
) -> Result<String> {
    let _feedback_job = state.feedback_job_guard();
    let agent_req = parse_feedback_agent_request(req).await?;

    let agents = discover_installed_agents();
    let agent = select_feedback_agent(&agents, agent_req.agent.as_deref())
        .ok_or_else(|| anyhow::anyhow!("No supported coding agent was found on PATH."))?;
    let canonical = agent.canonical.to_string();
    let label = agent.label.to_string();
    let executable = agent
        .executable_path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Selected agent is not available on PATH."))?;
    let prompt = build_feedback_prompt(report);
    let target_path = std::path::PathBuf::from(&report.target_path);

    let draft = run_feedback_agent(&canonical, &executable, &prompt, &target_path).await?;
    let message = parse_agent_feedback_message(&draft, Some(&target_path))?;
    let body = submit_report_feedback(FEEDBACK_CATEGORY.to_string(), message.clone()).await?;
    let message = format!(
        "Codemod name: {}\nCodemod version: {}\n\n{}",
        report.codemod_name,
        report.codemod_version.clone().unwrap_or("N/A".to_string()),
        message
    );
    let mut value: serde_json::Value = serde_json::from_str(&body)?;
    value["message"] = serde_json::Value::String(message);
    value["agent"] = serde_json::json!({
        "canonical": canonical,
        "label": label,
    });
    Ok(value.to_string())
}

async fn handle_feedback_agent_stream(
    req: Request<Body>,
    report: Arc<ExecutionReport>,
    state: Arc<ReportServerState>,
) -> Result<Response<Body>> {
    let agent_req = parse_feedback_agent_request(req).await?;

    let agents = discover_installed_agents();
    let agent = select_feedback_agent(&agents, agent_req.agent.as_deref())
        .ok_or_else(|| anyhow::anyhow!("No supported coding agent was found on PATH."))?;
    let canonical = agent.canonical.to_string();
    let label = agent.label.to_string();
    let executable = agent
        .executable_path
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Selected agent is not available on PATH."))?;
    let prompt = build_feedback_prompt(&report);
    let target_path = std::path::PathBuf::from(&report.target_path);

    let (mut sender, body) = Body::channel();
    let feedback_job = state.feedback_job_guard();
    tokio::spawn(async move {
        let _feedback_job = feedback_job;
        let agent_value = serde_json::json!({
            "canonical": canonical,
            "label": label,
        });
        let _ = send_stream_event(
            &mut sender,
            serde_json::json!({
                "type": "agent",
                "agent": agent_value,
            }),
        )
        .await;

        let draft =
            run_feedback_agent_stream(&canonical, &executable, &prompt, &target_path, &mut sender)
                .await;

        match draft {
            Ok(draft) => match parse_agent_feedback_message(&draft, Some(&target_path)) {
                Ok(message) => {
                    let _ = send_stream_event(
                        &mut sender,
                        serde_json::json!({
                            "type": "done",
                            "message": message,
                            "agent": agent_value,
                        }),
                    )
                    .await;
                }
                Err(error) => {
                    let _ = send_stream_error(&mut sender, error).await;
                }
            },
            Err(error) => {
                let _ = send_stream_error(&mut sender, error).await;
            }
        }
    });

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/x-ndjson")
        .header("Cache-Control", "no-cache")
        .body(body)
        .unwrap())
}

async fn parse_feedback_agent_request(req: Request<Body>) -> Result<FeedbackAgentRequest> {
    validate_feedback_request_headers(req.headers())?;

    let body_bytes = to_bytes(req.into_body()).await?;
    if body_bytes.is_empty() {
        anyhow::bail!("Feedback agent requests must include a JSON body.");
    }

    serde_json::from_slice(&body_bytes)
        .map_err(|error| anyhow::anyhow!("Invalid feedback agent request JSON: {error}"))
}

fn validate_feedback_request_headers(headers: &HeaderMap) -> Result<()> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let media_type = content_type.split(';').next().map(str::trim).unwrap_or("");
    if !media_type.eq_ignore_ascii_case("application/json") {
        anyhow::bail!("Feedback requests must use application/json.");
    }

    let Some(origin) = headers.get(ORIGIN) else {
        return Ok(());
    };
    let origin = origin
        .to_str()
        .map_err(|_| anyhow::anyhow!("Invalid Origin header."))?;
    let host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("Missing Host header."))?;

    if origin != format!("http://{host}") {
        anyhow::bail!("Feedback requests must come from the report UI origin.");
    }

    Ok(())
}

async fn submit_report_feedback(category: String, message: String) -> Result<String> {
    let category = sanitize_feedback_category(&category)?;
    let message = sanitize_feedback_message(&message)?;

    let consented_at = feedback::persist_feedback_consent()?;
    feedback::submit_anonymous_feedback(category.clone(), message.clone()).await?;

    Ok(serde_json::json!({
        "submitted": true,
        "category": category,
        "message": message,
        "consentedAt": consented_at,
    })
    .to_string())
}

fn sanitize_feedback_category(category: &str) -> Result<String> {
    let category = category.trim();
    if category.is_empty() {
        anyhow::bail!("Feedback category cannot be empty.");
    }
    if category.len() > 64
        || !category.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        anyhow::bail!("Feedback category must be 64 characters or fewer and contain only letters, numbers, '.', '_', or '-'.");
    }
    Ok(category.to_string())
}

fn sanitize_feedback_message(message: &str) -> Result<String> {
    let message = console::strip_ansi_codes(message).trim().to_string();
    if message.is_empty() {
        anyhow::bail!("Feedback message cannot be empty.");
    }
    if message.len() > MAX_FEEDBACK_MESSAGE_LEN {
        anyhow::bail!("Feedback message must be {MAX_FEEDBACK_MESSAGE_LEN} characters or fewer.");
    }
    Ok(message)
}

fn select_feedback_agent<'a>(
    agents: &'a [AgentOption],
    requested: Option<&str>,
) -> Option<&'a AgentOption> {
    if let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        return agents
            .iter()
            .find(|agent| agent.canonical == requested && agent.is_available());
    }

    const PREFERRED: &[&str] = &["claude-code", "codex", "opencode", "goose"];
    PREFERRED.iter().find_map(|canonical| {
        agents
            .iter()
            .find(|agent| agent.canonical == *canonical && agent.is_available())
    })
}

fn build_feedback_prompt(report: &ExecutionReport) -> String {
    let total_metric_entries = report
        .metrics
        .values()
        .map(std::vec::Vec::len)
        .sum::<usize>();
    let largest_files = report
        .diffs
        .iter()
        .take(12)
        .map(|diff| {
            serde_json::json!({
                "extension": std::path::Path::new(&diff.path)
                    .extension()
                    .and_then(std::ffi::OsStr::to_str)
                    .unwrap_or(""),
                "additions": diff.additions,
                "deletions": diff.deletions,
            })
        })
        .collect::<Vec<_>>();
    let metric_names = report.metrics.keys().take(20).collect::<Vec<_>>();
    let summary = serde_json::json!({
        "codemodName": &report.codemod_name,
        "codemodVersion": &report.codemod_version,
        "durationMs": report.duration_ms,
        "dryRun": report.dry_run,
        "targetPath": &report.target_path,
        "stats": &report.stats,
        "metricNames": metric_names,
        "metricEntryCount": total_metric_entries,
        "changedFileCount": report.diffs.len(),
        "changedFileShapeSample": largest_files,
        "stepCount": report.diff_groups.len(),
    });

    format!(
        r#"You are helping improve Codemod's codemod execution quality.

Create anonymous product feedback about how this codemod performed.

Use the targetPath and sanitized execution summary below to understand the run. You may inspect the targetPath with read-only actions such as listing files, reading files, searching, and checking git status or git diff. Do not edit files, write files, run formatters, run package installs, run tests that mutate state, submit code, include source code, include local paths, include secrets, include auth tokens, identify the user, or include long transcripts.

Return exactly one JSON object and no markdown:
{{"message":"one concise paragraph, 3-5 sentences, under 1200 characters, that summarizes what the codemod changed and highlights shortcomings or risks Codemod should improve. Do not include code snippets, file contents, local paths, user identity, or secrets."}}

Execution context:
{}"#,
        summary
    )
}

async fn run_feedback_agent(
    canonical: &str,
    executable: &std::path::Path,
    prompt: &str,
    working_dir: &std::path::Path,
) -> Result<String> {
    let mut cmd = tokio::process::Command::new(executable);
    cmd.current_dir(resolve_feedback_agent_working_dir(working_dir));
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    match canonical {
        "claude-code" => {
            cmd.arg("-p")
                .arg("--output-format")
                .arg("stream-json")
                .arg("--verbose")
                .stdin(Stdio::piped());
        }
        "codex" => {
            cmd.env("RUST_LOG", "error");
            cmd.arg("e").arg("--json").arg("-").stdin(Stdio::piped());
        }
        "opencode" => {
            cmd.arg("run")
                .arg("--format")
                .arg("json")
                .stdin(Stdio::piped());
        }
        "goose" => {
            cmd.env("GOOSE_MODE", "auto")
                .arg("run")
                .arg("--text")
                .arg(prompt)
                .stdin(Stdio::null());
        }
        other => anyhow::bail!("Agent '{other}' is not supported for report feedback."),
    }

    let mut child = cmd.spawn()?;
    if matches!(canonical, "claude-code" | "codex" | "opencode") {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Agent stdin pipe was not available."))?;
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.shutdown().await?;
    }

    let output = timeout(FEEDBACK_AGENT_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("Agent feedback drafting timed out."))??;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    if !output.status.success() {
        anyhow::bail!("Agent feedback drafting failed: {}", combined.trim());
    }

    Ok(combined)
}

async fn run_feedback_agent_stream(
    canonical: &str,
    executable: &std::path::Path,
    prompt: &str,
    working_dir: &std::path::Path,
    sender: &mut hyper::body::Sender,
) -> Result<String> {
    let mut cmd = tokio::process::Command::new(executable);
    cmd.current_dir(resolve_feedback_agent_working_dir(working_dir));
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    match canonical {
        "claude-code" => {
            cmd.arg("-p")
                .arg("--output-format")
                .arg("stream-json")
                .arg("--verbose")
                .stdin(Stdio::piped());
        }
        "codex" => {
            cmd.env("RUST_LOG", "error");
            cmd.arg("e").arg("--json").arg("-").stdin(Stdio::piped());
        }
        "opencode" => {
            cmd.arg("run")
                .arg("--format")
                .arg("json")
                .stdin(Stdio::piped());
        }
        "goose" => {
            cmd.env("GOOSE_MODE", "auto")
                .arg("run")
                .arg("--text")
                .arg(prompt)
                .stdin(Stdio::null());
        }
        other => anyhow::bail!("Agent '{other}' is not supported for report feedback."),
    }

    let mut child = cmd.spawn()?;
    if matches!(canonical, "claude-code" | "codex" | "opencode") {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Agent stdin pipe was not available."))?;
        stdin.write_all(prompt.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.shutdown().await?;
    }

    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<(String, String)>(64);
    let stdout_reader = child
        .stdout
        .take()
        .map(|stdout| spawn_agent_stream_reader(stdout, "stdout", line_tx.clone()));
    let stderr_reader = child
        .stderr
        .take()
        .map(|stderr| spawn_agent_stream_reader(stderr, "stderr", line_tx.clone()));
    drop(line_tx);

    let mut wait_status = None;
    let mut lines_closed = false;
    let mut combined = String::new();
    let timeout_timer = sleep(FEEDBACK_AGENT_TIMEOUT);
    tokio::pin!(timeout_timer);

    while wait_status.is_none() || !lines_closed {
        enum AgentStreamStep {
            Line(Option<(String, String)>),
            Exit(std::io::Result<std::process::ExitStatus>),
            Timeout,
        }

        let step = if wait_status.is_none() {
            let wait = child.wait();
            tokio::pin!(wait);
            tokio::select! {
                line = line_rx.recv(), if !lines_closed => AgentStreamStep::Line(line),
                status = &mut wait => AgentStreamStep::Exit(status),
                _ = &mut timeout_timer => AgentStreamStep::Timeout,
            }
        } else {
            tokio::select! {
                line = line_rx.recv(), if !lines_closed => AgentStreamStep::Line(line),
                _ = &mut timeout_timer => AgentStreamStep::Timeout,
            }
        };

        match step {
            AgentStreamStep::Line(line) => match line {
                Some((stream, line)) => {
                    combined.push_str(&line);
                    combined.push('\n');
                    for event in feedback_stream_events_for_agent_line(canonical, &stream, &line) {
                        send_stream_event(sender, event).await?;
                    }
                }
                None => {
                    lines_closed = true;
                }
            },
            AgentStreamStep::Exit(status) => {
                wait_status = Some(status?);
            }
            AgentStreamStep::Timeout => {
                let _ = send_stream_event(
                    sender,
                    serde_json::json!({
                        "type": "status",
                        "message": "Agent feedback drafting timed out; stopping the local agent.",
                    }),
                )
                .await;
                let _ = child.kill().await;
                if let Some(reader) = &stdout_reader {
                    reader.abort();
                }
                if let Some(reader) = &stderr_reader {
                    reader.abort();
                }
                anyhow::bail!("Agent feedback drafting timed out.");
            }
        }

        while wait_status.is_some() && !lines_closed {
            match line_rx.try_recv() {
                Ok((stream, line)) => {
                    combined.push_str(&line);
                    combined.push('\n');
                    for event in feedback_stream_events_for_agent_line(canonical, &stream, &line) {
                        send_stream_event(sender, event).await?;
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    break;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    lines_closed = true;
                    break;
                }
            }
        }
    }

    if let Some(reader) = stdout_reader {
        let _ = reader.await;
    }
    if let Some(reader) = stderr_reader {
        let _ = reader.await;
    }

    let status = wait_status.ok_or_else(|| anyhow::anyhow!("Agent exited without a status."))?;
    if !status.success() {
        anyhow::bail!("Agent feedback drafting failed: {}", combined.trim());
    }

    Ok(combined)
}

fn spawn_agent_stream_reader<R>(
    reader: R,
    stream: &'static str,
    tx: tokio::sync::mpsc::Sender<(String, String)>,
) -> tokio::task::JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            if tx.send((stream.to_string(), line)).await.is_err() {
                break;
            }
        }
    })
}

async fn send_stream_event(
    sender: &mut hyper::body::Sender,
    event: serde_json::Value,
) -> Result<()> {
    let line = format!("{event}\n");
    sender
        .send_data(line.into())
        .await
        .map_err(|error| anyhow::anyhow!("Failed to stream feedback event: {error}"))
}

async fn send_stream_error(sender: &mut hyper::body::Sender, error: anyhow::Error) -> Result<()> {
    send_stream_event(
        sender,
        serde_json::json!({
            "type": "error",
            "error": error.to_string(),
        }),
    )
    .await
}

fn feedback_stream_events_for_agent_line(
    canonical: &str,
    stream: &str,
    line: &str,
) -> Vec<serde_json::Value> {
    let mut events = Vec::new();

    if let Some(message) = describe_agent_activity_line(canonical, line) {
        events.push(serde_json::json!({
            "type": "status",
            "message": message,
        }));
    }

    let text = extract_agent_text(line);
    let text = text.trim();
    if !text.is_empty() && text != line.trim() {
        events.push(serde_json::json!({
            "type": "output",
            "stream": stream,
            "text": text,
        }));
    } else if serde_json::from_str::<serde_json::Value>(line).is_err() {
        events.push(serde_json::json!({
            "type": "status",
            "message": format!("[{stream}] {}", truncate_feedback_stream_line(line, 240)),
        }));
    }

    events
}

fn describe_agent_activity_line(canonical: &str, line: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
    match canonical {
        "claude-code" => describe_claude_activity(&value),
        "codex" => describe_codex_activity(&value),
        "opencode" => describe_opencode_activity(&value),
        _ => None,
    }
}

fn describe_claude_activity(value: &serde_json::Value) -> Option<String> {
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("system")
            if value.get("subtype").and_then(serde_json::Value::as_str) == Some("init") =>
        {
            let model = value
                .get("model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("model");
            Some(format!("Claude Code started ({model})."))
        }
        Some("stream_event") => {
            let event = value.get("event")?;
            if event.get("type").and_then(serde_json::Value::as_str) == Some("content_block_start")
            {
                let block = event.get("content_block")?;
                if block.get("type").and_then(serde_json::Value::as_str) == Some("tool_use") {
                    let name = block
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("tool");
                    return Some(format!("Using {name}."));
                }
            }
            None
        }
        _ => None,
    }
}

fn describe_codex_activity(value: &serde_json::Value) -> Option<String> {
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("turn.started") => Some("Codex started drafting feedback.".to_string()),
        Some("turn.completed") => Some("Codex finished drafting feedback.".to_string()),
        Some("item.started") => {
            let item = value.get("item")?;
            match item.get("type").and_then(serde_json::Value::as_str) {
                Some("command_execution") => {
                    let command = item
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("command");
                    Some(format!(
                        "Running read-only command: {}",
                        truncate_feedback_stream_line(command, 180)
                    ))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn describe_opencode_activity(value: &serde_json::Value) -> Option<String> {
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("agent") => value
            .pointer("/part/name")
            .or_else(|| value.pointer("/properties/name"))
            .and_then(serde_json::Value::as_str)
            .map(|name| format!("OpenCode agent: {name}.")),
        Some("tool") | Some("tool_use") => value
            .pointer("/part/tool")
            .or_else(|| value.pointer("/properties/tool"))
            .and_then(serde_json::Value::as_str)
            .map(|tool| format!("Using {tool}.")),
        _ => None,
    }
}

fn truncate_feedback_stream_line(line: &str, max_chars: usize) -> String {
    let mut truncated = String::new();
    for (index, ch) in line.chars().enumerate() {
        if index >= max_chars {
            truncated.push_str("...");
            return truncated;
        }
        truncated.push(ch);
    }
    truncated
}

fn resolve_feedback_agent_working_dir(working_dir: &std::path::Path) -> std::path::PathBuf {
    if working_dir.as_os_str().is_empty() {
        return std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    }

    if working_dir.is_file() {
        return working_dir
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
    }

    working_dir.to_path_buf()
}

fn parse_agent_feedback_message(
    output: &str,
    target_path: Option<&std::path::Path>,
) -> Result<String> {
    let text = extract_agent_text(output);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        if let Some(message) = value.get("message").and_then(serde_json::Value::as_str) {
            return sanitize_agent_feedback_message(message, target_path);
        }
    }

    if let Some(json_text) = extract_json_object(text.trim()) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_text) {
            if let Some(message) = value.get("message").and_then(serde_json::Value::as_str) {
                return sanitize_agent_feedback_message(message, target_path);
            }
        }
    }

    sanitize_agent_feedback_message(&text, target_path)
}

fn sanitize_agent_feedback_message(
    message: &str,
    target_path: Option<&std::path::Path>,
) -> Result<String> {
    let without_code_blocks = strip_markdown_code_blocks(message);
    let redacted = redact_feedback_paths(&without_code_blocks, target_path);
    sanitize_feedback_message(&redacted)
}

fn strip_markdown_code_blocks(message: &str) -> String {
    let mut stripped = String::new();
    let mut in_code_block = false;

    for line in message.lines() {
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        if !in_code_block {
            stripped.push_str(line);
            stripped.push('\n');
        }
    }

    stripped
}

fn redact_feedback_paths(message: &str, target_path: Option<&std::path::Path>) -> String {
    let Some(target_path) = target_path else {
        return message.to_string();
    };
    let target = target_path.display().to_string();
    if target.trim().is_empty() {
        return message.to_string();
    }

    message.replace(&target, "[target path]")
}

fn extract_agent_text(output: &str) -> String {
    let mut collected = String::new();
    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            collected.push_str(line);
            collected.push('\n');
            continue;
        };
        collect_agent_text_from_value(&value, &mut collected);
    }

    if collected.trim().is_empty() {
        output.to_string()
    } else {
        collected
    }
}

fn collect_agent_text_from_value(value: &serde_json::Value, collected: &mut String) {
    if let Some(text) = value
        .pointer("/event/delta/text")
        .and_then(serde_json::Value::as_str)
    {
        collected.push_str(text);
    }
    if let Some(text) = value
        .pointer("/event/delta/partial_json")
        .and_then(serde_json::Value::as_str)
    {
        collected.push_str(text);
    }
    if let Some(text) = value.get("text").and_then(serde_json::Value::as_str) {
        collected.push_str(text);
        collected.push('\n');
    }
    if let Some(text) = value.get("message").and_then(serde_json::Value::as_str) {
        collected.push_str(text);
        collected.push('\n');
    }
    if let Some(text) = value
        .pointer("/item/text")
        .and_then(serde_json::Value::as_str)
    {
        collected.push_str(text);
        collected.push('\n');
    }
    if let Some(text) = value
        .pointer("/part/text")
        .and_then(serde_json::Value::as_str)
    {
        collected.push_str(text);
        collected.push('\n');
    }

    if let Some(content) = value.get("content").and_then(serde_json::Value::as_array) {
        for item in content {
            collect_agent_text_from_value(item, collected);
        }
    }
    if let Some(content) = value
        .pointer("/message/content")
        .and_then(serde_json::Value::as_array)
    {
        for item in content {
            collect_agent_text_from_value(item, collected);
        }
    }
}

fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (start <= end).then_some(&text[start..=end])
}

/// Handle login request — triggers OIDC flow from the report server
async fn handle_login() -> Result<String> {
    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;
    let registry_url = config.default_registry.clone();

    let registry_config = config
        .registries
        .get(&registry_url)
        .ok_or_else(|| anyhow::anyhow!("Unknown registry: {}", registry_url))?
        .clone();

    let oidc_client = OidcClient::new(registry_url, registry_config)?;
    let auth = oidc_client.login().await?;

    Ok(serde_json::json!({
        "success": true,
        "username": auth.user.username,
    })
    .to_string())
}

/// Serve the execution report on a local HTTP server and open the browser
pub async fn serve_report(report: ExecutionReport) -> Result<()> {
    let port = find_available_port(9100, 9200)
        .ok_or_else(|| anyhow::anyhow!("No available port found in range 9100-9200"))?;

    let report = Arc::new(report);
    let server_state = Arc::new(ReportServerState::new());
    let addr = ([127, 0, 0, 1], port).into();

    let service_state = server_state.clone();
    let make_svc = make_service_fn(move |_conn| {
        let report = report.clone();
        let server_state = service_state.clone();
        async move {
            Ok::<_, Infallible>(service_fn(move |req| {
                let report = report.clone();
                let server_state = server_state.clone();
                handle_request(req, report, server_state)
            }))
        }
    });

    let server = Server::bind(&addr).serve(make_svc);

    let url = format!("http://127.0.0.1:{port}");
    println!("\n📊 Report available at: {url}");

    if let Err(e) = open::that(&url) {
        eprintln!("Failed to open browser: {e}. Open {url} manually.");
    }

    let idle_state = server_state.clone();
    let graceful = server.with_graceful_shutdown(async move {
        loop {
            tokio::time::sleep(REPORT_SERVER_IDLE_CHECK_INTERVAL).await;
            if idle_state.has_active_feedback_jobs() {
                continue;
            }
            if idle_state.idle_for() >= REPORT_SERVER_IDLE_TIMEOUT {
                println!("\nReport server shutting down after 5 minutes of inactivity.");
                break;
            }
        }
    });

    tokio::select! {
        result = graceful => {
            if let Err(e) = result {
                eprintln!("Report server error: {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\nReport server shutting down.");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterflow_core::report::{
        ReportDiffGroup, ReportFileDiff, ReportMetricEntry, ReportStats,
    };
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn unavailable_agent(canonical: &'static str, label: &'static str) -> AgentOption {
        AgentOption {
            canonical,
            label,
            executable_path: None,
        }
    }

    fn available_agent(canonical: &'static str, label: &'static str) -> AgentOption {
        AgentOption {
            canonical,
            label,
            executable_path: Some(PathBuf::from("/bin/agent")),
        }
    }

    fn sample_report() -> ExecutionReport {
        let mut metrics = HashMap::new();
        metrics.insert(
            "renamedImports".to_string(),
            vec![ReportMetricEntry {
                cardinality: HashMap::new(),
                count: 3,
            }],
        );

        ExecutionReport {
            version: 2,
            id: "report-id".to_string(),
            codemod_name: "@org/private-codemod".to_string(),
            codemod_version: Some("1.0.0".to_string()),
            executed_at: "2026-06-25T00:00:00Z".to_string(),
            duration_ms: 1200.0,
            dry_run: false,
            target_path: "/Users/me/private/project".to_string(),
            cli_version: "0.0.0".to_string(),
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            stats: ReportStats {
                files_modified: 1,
                files_unmodified: 2,
                files_with_errors: 1,
                total_additions: 4,
                total_deletions: 2,
            },
            metrics,
            diffs: vec![ReportFileDiff {
                path: "src/private/path.ts".to_string(),
                diff_text: Some("secret source code".to_string()),
                additions: 4,
                deletions: 2,
                step_id: Some("step-a".to_string()),
                step_name: Some("Private step".to_string()),
            }],
            diff_groups: vec![ReportDiffGroup {
                step_id: Some("step-a".to_string()),
                step_name: "Private step".to_string(),
                additions: 4,
                deletions: 2,
                diffs: Vec::new(),
            }],
            registry_link_url: None,
        }
    }

    #[test]
    fn selects_first_supported_available_agent_by_preference() {
        let agents = vec![
            unavailable_agent("claude-code", "Claude Code"),
            available_agent("codex", "Codex"),
            available_agent("goose", "Goose"),
        ];

        let selected = select_feedback_agent(&agents, None).expect("selected agent");

        assert_eq!(selected.canonical, "codex");
    }

    #[test]
    fn requested_agent_must_be_available() {
        let agents = vec![
            unavailable_agent("codex", "Codex"),
            available_agent("goose", "Goose"),
        ];

        assert!(select_feedback_agent(&agents, Some("codex")).is_none());
    }

    #[test]
    fn agent_message_can_be_extracted_from_codex_json_events() {
        let output = r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"message\":\"Changed one file and surfaced missing coverage.\"}"}}"#;

        let message = parse_agent_feedback_message(output, None).expect("parsed message");

        assert_eq!(message, "Changed one file and surfaced missing coverage.");
    }

    #[test]
    fn feedback_prompt_includes_target_path_but_omits_diff_text() {
        let prompt = build_feedback_prompt(&sample_report());

        assert!(prompt.contains("@org/private-codemod"));
        assert!(prompt.contains("/Users/me/private/project"));
        assert!(prompt.contains("read-only actions"));
        assert!(prompt.contains("\"extension\":\"ts\""));
        assert!(!prompt.contains("src/private/path.ts"));
        assert!(!prompt.contains("secret source code"));
        assert!(!prompt.contains("Private step"));
    }

    #[test]
    fn feedback_message_validation_strips_ansi_and_rejects_empty() {
        let message = sanitize_feedback_message("\u{1b}[32mUseful feedback\u{1b}[0m")
            .expect("valid feedback");

        assert_eq!(message, "Useful feedback");
        assert!(sanitize_feedback_message("   ").is_err());
    }

    #[test]
    fn agent_feedback_sanitization_removes_code_blocks_and_redacts_target_path() {
        let message = sanitize_agent_feedback_message(
            "Changed files under /Users/me/private/project.\n```ts\nconst secret = true;\n```\nNeeds better error reporting.",
            Some(std::path::Path::new("/Users/me/private/project")),
        )
        .expect("sanitized message");

        assert_eq!(
            message,
            "Changed files under [target path].\nNeeds better error reporting."
        );
        assert!(!message.contains("const secret"));
        assert!(!message.contains("/Users/me/private/project"));
    }

    #[test]
    fn feedback_request_headers_require_json_content_type() {
        let headers = HeaderMap::new();

        assert!(validate_feedback_request_headers(&headers).is_err());
    }

    #[test]
    fn feedback_request_headers_reject_cross_origin_requests() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
        headers.insert(HOST, "127.0.0.1:4321".parse().unwrap());
        headers.insert(ORIGIN, "https://example.com".parse().unwrap());

        assert!(validate_feedback_request_headers(&headers).is_err());
    }

    #[test]
    fn feedback_request_headers_allow_report_ui_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        headers.insert(HOST, "127.0.0.1:4321".parse().unwrap());
        headers.insert(ORIGIN, "http://127.0.0.1:4321".parse().unwrap());

        validate_feedback_request_headers(&headers).expect("valid feedback request headers");
    }
}

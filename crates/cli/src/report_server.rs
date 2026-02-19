use anyhow::Result;
use butterflow_core::report::{ExecutionReport, ShareLevel};
use hyper::body::to_bytes;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use std::convert::Infallible;
use std::net::TcpListener;
use std::sync::Arc;

use crate::auth::{OidcClient, TokenStorage};

/// Embedded report HTML (built by Preact SPA in report-ui/)
#[cfg(not(debug_assertions))]
const REPORT_HTML: &str = include_str!("../report-ui/dist/index.html");

/// In debug builds, try to read the built SPA from disk at runtime
#[cfg(debug_assertions)]
fn get_report_html() -> String {
    let candidates = [
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
) -> std::result::Result<Response<Body>, Infallible> {
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

fn default_share_level() -> ShareLevel {
    ShareLevel::WithFiles
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
    let addr = ([127, 0, 0, 1], port).into();

    let make_svc = make_service_fn(move |_conn| {
        let report = report.clone();
        async move {
            Ok::<_, Infallible>(service_fn(move |req| {
                let report = report.clone();
                handle_request(req, report)
            }))
        }
    });

    let server = Server::bind(&addr).serve(make_svc);

    let url = format!("http://127.0.0.1:{port}");
    println!("\n📊 Report available at: {url}");

    if let Err(e) = open::that(&url) {
        eprintln!("Failed to open browser: {e}. Open {url} manually.");
    }

    let graceful = server.with_graceful_shutdown(async {
        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
        println!("\nReport server shutting down after 5 minutes of inactivity.");
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

//! `http(url, { method, headers, body, json, timeout, allowErrorStatus, dryRun })`.
//!
//! Callbacks have no network: a workflow that needs one declares the request
//! as a step. The response is the step's result, so it is recorded with the
//! run and a resumed run gets it back without sending the request again.
//!
//! Header values given as `{ env: "NAME" }` are read from the environment
//! when the request is sent; the run records the name, never the value.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{block_on, TaskContext, TaskError, TaskOutcome, TaskResult};

/// The most of a response body a step keeps: the run records it.
const MAX_BODY: usize = 10 * 1024 * 1024;

/// How long a request may take when the step gives no `timeout`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum HeaderValue {
    Text(String),
    Env { env: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Request {
    url: String,
    method: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, HeaderValue>,
    body: Option<String>,
    json: Option<Value>,
    /// Milliseconds.
    timeout: Option<u64>,
    /// A 4xx or 5xx response is the result instead of failing the step.
    #[serde(default)]
    allow_error_status: bool,
    /// `"perform"`: send the request in a dry run too.
    dry_run: Option<String>,
}

impl Request {
    fn method(&self) -> String {
        self.method.as_deref().unwrap_or("GET").to_ascii_uppercase()
    }
}

pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    let method = request.method();
    if task.run.dry_run && request.dry_run.as_deref() != Some("perform") {
        task.log(format!(
            "skipped: would request {method} {} (http steps do not run in a dry run unless they set dryRun: \"perform\")",
            request.url
        ));
        return Ok(TaskOutcome::new(json!({ "skipped": true })));
    }
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| TaskError::Failed(format!("'{method}' is not an HTTP method")))?;
    let url = reqwest::Url::parse(&request.url)
        .map_err(|e| TaskError::Failed(format!("invalid url '{}': {e}", request.url)))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(TaskError::Failed(format!(
            "http steps request http:// or https:// URLs, not '{}'",
            request.url
        )));
    }

    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in &request.headers {
        let value = match value {
            HeaderValue::Text(text) => text.clone(),
            HeaderValue::Env { env } => std::env::var(env).map_err(|_| {
                TaskError::Failed(format!(
                    "header '{name}' reads the environment variable {env}, which is not set"
                ))
            })?,
        };
        let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| TaskError::Failed(format!("'{name}' is not a valid header name")))?;
        let header_value = reqwest::header::HeaderValue::from_str(&value)
            .map_err(|_| TaskError::Failed(format!("header '{name}' has an invalid value")))?;
        headers.insert(header_name, header_value);
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("codemod-workflow/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| TaskError::Failed(format!("cannot make an HTTP client: {e}")))?;
    let timeout = request
        .timeout
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TIMEOUT);
    let mut builder = client
        .request(method.clone(), url.clone())
        .headers(headers)
        .timeout(timeout);
    if let Some(body) = &request.body {
        builder = builder.body(body.clone());
    }
    if let Some(value) = &request.json {
        builder = builder.json(value);
    }

    task.log(format!("{method} {url}"));
    let cancel = std::sync::Arc::clone(&task.cancel);
    let response = block_on(async move {
        let canceled = async {
            while !cancel.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        tokio::select! {
            response = send(builder) => response.map(Some),
            _ = canceled => Ok(None),
        }
    });
    let Some(response) = response? else {
        return Err(TaskError::Canceled);
    };
    let status = response.status;
    task.log(format!("{} {}", status, response.status_text));
    if status >= 400 && !request.allow_error_status {
        let text = response
            .result
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let excerpt: String = text.chars().take(500).collect();
        return Err(TaskError::Failed(format!(
            "{method} {url} answered {status} {}{}",
            response.status_text,
            if excerpt.trim().is_empty() {
                String::new()
            } else {
                format!(":\n{excerpt}")
            }
        )));
    }
    Ok(TaskOutcome::new(response.result))
}

struct Response {
    status: u16,
    status_text: String,
    result: Value,
}

async fn send(builder: reqwest::RequestBuilder) -> Result<Response, TaskError> {
    let response = builder.send().await.map_err(|e| {
        TaskError::Failed(if e.is_timeout() {
            "the request timed out".to_string()
        } else {
            format!("the request failed: {e}")
        })
    })?;
    let status = response.status();
    let final_url = response.url().to_string();
    let mut headers = Map::new();
    for (name, value) in response.headers() {
        let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
        // Repeated headers are joined, as fetch's Headers does.
        match headers.get_mut(name.as_str()) {
            Some(Value::String(existing)) => {
                existing.push_str(", ");
                existing.push_str(&value);
            }
            _ => {
                headers.insert(name.as_str().to_string(), Value::String(value));
            }
        }
    }
    let is_json = headers
        .get("content-type")
        .and_then(Value::as_str)
        .map(|kind| {
            let kind = kind.split(';').next().unwrap_or_default().trim();
            kind.eq_ignore_ascii_case("application/json") || kind.ends_with("+json")
        })
        .unwrap_or(false);

    let mut bytes = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| TaskError::Failed(format!("reading the response failed: {e}")))?
    {
        if bytes.len() + chunk.len() > MAX_BODY {
            return Err(TaskError::Failed(format!(
                "the response is larger than {} MiB, the most an http step records",
                MAX_BODY / (1024 * 1024)
            )));
        }
        bytes.extend_from_slice(&chunk);
    }

    let status_text = status.canonical_reason().unwrap_or_default().to_string();
    let mut result = json!({
        "status": status.as_u16(),
        "statusText": status_text,
        "ok": status.is_success(),
        "url": final_url,
        "headers": Value::Object(headers),
    });
    match String::from_utf8(bytes) {
        Ok(text) => {
            if is_json {
                if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                    result["json"] = parsed;
                }
            }
            result["body"] = Value::String(text);
        }
        Err(error) => {
            result["body"] =
                Value::String(base64::engine::general_purpose::STANDARD.encode(error.into_bytes()));
            result["bodyEncoding"] = Value::String("base64".into());
        }
    }
    Ok(Response {
        status: status.as_u16(),
        status_text,
        result,
    })
}

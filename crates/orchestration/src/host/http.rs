//! `httpRequest`: the one network primitive of the QuickJS host, for the
//! `assessment` operation (the decision transport runs in the bundle with a
//! `fetch` built on it, `src/host/quickjs/fetch.ts`).
//!
//! It is deliberately not a general `fetch`: workflow and transform code
//! share this context, and the runtime is the sandboxed one. A request is
//! allowed only under the decision API root the run was started with, resolved
//! as `src/core/decision.ts` resolves it: `DECISION_BASE_URL`, else (for the
//! `systemone` provider) `TYPESAFE_BASE_URL`, else the provider's default.
//! Same scheme, host, and port, and a path under the root's path. Redirects
//! are not followed, so they cannot leave it either.
//!
//! The request runs on a helper thread (reqwest's blocking client); the
//! promise settles on the JS thread through the tokio driver, like `spawn`.
//! Abort is the caller's: the JS side rejects at once and the thread finishes
//! within `timeoutMs`.

use std::collections::HashMap;
use std::time::Duration;

use reqwest::Url;
use rquickjs::{Ctx, Function, Object, Promise};

use crate::host::native::throw;

pub const PROVIDER_ENV: &str = "DECISION_PROVIDER";
pub const BASE_URL_ENV: &str = "DECISION_BASE_URL";
/// Read for the `systemone` provider when `DECISION_BASE_URL` is unset.
pub const LEGACY_BASE_URL_ENV: &str = "TYPESAFE_BASE_URL";
/// Largest response body accepted (the decision transport reads it whole).
const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// The allowed API root for the configured provider. Blank values count as unset.
pub fn allowed_base(env: &HashMap<String, String>) -> Result<Url, String> {
    let setting = |name: &str| {
        env.get(name)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    };
    let raw = match setting(PROVIDER_ENV).unwrap_or("systemone") {
        "systemone" => setting(BASE_URL_ENV)
            .or_else(|| setting(LEGACY_BASE_URL_ENV))
            .unwrap_or("https://api.typesafe.ai"),
        "openai" => setting(BASE_URL_ENV).unwrap_or("https://api.openai.com"),
        other => {
            return Err(format!(
                "{PROVIDER_ENV} '{other}' is not one of systemone, openai"
            ))
        }
    };
    let url = Url::parse(raw)
        .map_err(|error| format!("decision base URL '{raw}' is invalid: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("decision base URL '{raw}' is not an http(s) URL"));
    }
    Ok(url)
}

/// Whether `url` is under `base`: same origin, and the path is the base path or below it.
pub fn is_allowed(base: &Url, url: &Url) -> bool {
    if url.scheme() != base.scheme()
        || url.host_str() != base.host_str()
        || url.port_or_known_default() != base.port_or_known_default()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let root = base.path().trim_end_matches('/');
    let path = url.path();
    path == root || path.starts_with(&format!("{root}/")) || root.is_empty()
}

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

fn send(
    url: Url,
    method: &str,
    headers: HashMap<String, String>,
    body: Option<String>,
    timeout: Duration,
) -> Result<Response, String> {
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| format!("invalid HTTP method '{method}'"))?;
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client.request(method, url);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.body(body);
    }
    let response = request.send().map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES)
    {
        return Err(format!("response body over {MAX_BODY_BYTES} bytes"));
    }
    let bytes = response.bytes().map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_BODY_BYTES {
        return Err(format!("response body over {MAX_BODY_BYTES} bytes"));
    }
    Ok(Response {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// `httpRequest({url, method, headers, body, timeoutMs})` -> `Promise<{status, headers, body}>`.
pub fn install<'js>(
    ctx: &Ctx<'js>,
    native: &Object<'js>,
    base: Result<Url, String>,
) -> rquickjs::Result<()> {
    native.set(
        "httpRequest",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, request: Object<'js>| -> rquickjs::Result<Promise<'js>> {
                let base = match &base {
                    Ok(base) => base.clone(),
                    Err(message) => return Err(throw(&ctx, Some("EINVAL"), message)),
                };
                let raw: String = request.get("url")?;
                let method: Option<String> = request.get("method")?;
                let headers: Option<HashMap<String, String>> = request.get("headers")?;
                let body: Option<String> = request.get("body")?;
                let timeout_ms: Option<f64> = request.get("timeoutMs")?;
                let url = Url::parse(&raw)
                    .map_err(|error| throw(&ctx, Some("EINVAL"), &format!("invalid URL '{raw}': {error}")))?;
                if !is_allowed(&base, &url) {
                    return Err(throw(
                        &ctx,
                        Some("EACCES"),
                        &format!("request to '{raw}' refused: the QuickJS host only reaches the decision API at '{base}' ({BASE_URL_ENV})"),
                    ));
                }
                let timeout = Duration::from_millis(
                    timeout_ms
                        .filter(|ms| ms.is_finite() && *ms >= 1.0)
                        .map_or(30_000, |ms| ms.min(2_147_483_647.0) as u64),
                );
                let method = method.unwrap_or_else(|| "GET".to_string());
                let headers = headers.unwrap_or_default();

                let (promise, resolve, reject) = Promise::new(&ctx)?;
                let (tx, rx) = tokio::sync::oneshot::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(send(url, &method, headers, body, timeout));
                });
                let ctx2 = ctx.clone();
                ctx.spawn(async move {
                    let outcome = rx
                        .await
                        .unwrap_or_else(|_| Err("request thread ended".to_string()));
                    let settled = (|| -> rquickjs::Result<()> {
                        match outcome {
                            Ok(response) => {
                                let result = Object::new(ctx2.clone())?;
                                result.set("status", response.status)?;
                                let headers = rquickjs::Array::new(ctx2.clone())?;
                                for (index, (name, value)) in response.headers.into_iter().enumerate() {
                                    headers.set(index, vec![name, value])?;
                                }
                                result.set("headers", headers)?;
                                result.set("body", response.body)?;
                                resolve.call::<_, ()>((result,))
                            }
                            Err(message) => {
                                let error = rquickjs::Exception::from_message(ctx2.clone(), &message)?;
                                error.as_object().set("code", "ECONNECTION")?;
                                reject.call::<_, ()>((error,))
                            }
                        }
                    })();
                    if settled.is_err() {
                        let _ = ctx2.catch();
                    }
                });
                Ok(promise)
            },
        )?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    #[test]
    fn allows_only_paths_under_the_base_root() {
        let base = url("http://127.0.0.1:8080/api");
        assert!(is_allowed(&base, &url("http://127.0.0.1:8080/api")));
        assert!(is_allowed(
            &base,
            &url("http://127.0.0.1:8080/api/v1/system-one?x=1")
        ));
        assert!(!is_allowed(&base, &url("http://127.0.0.1:8080/apix")));
        assert!(!is_allowed(&base, &url("http://127.0.0.1:8080/other")));
        assert!(!is_allowed(&base, &url("http://127.0.0.1:8081/api/v1")));
        assert!(!is_allowed(&base, &url("https://127.0.0.1:8080/api/v1")));
        assert!(!is_allowed(&base, &url("http://localhost:8080/api/v1")));
        assert!(!is_allowed(
            &base,
            &url("http://user:pw@127.0.0.1:8080/api/v1")
        ));

        let root = url("https://api.typesafe.ai");
        assert!(is_allowed(
            &root,
            &url("https://api.typesafe.ai/v1/system-one")
        ));
        assert!(is_allowed(&root, &url("https://api.typesafe.ai:443/v1")));
        assert!(!is_allowed(&root, &url("https://evil.example/v1")));
        assert!(!is_allowed(&root, &url("http://api.typesafe.ai/v1")));
    }

    #[test]
    fn base_follows_the_configured_provider() {
        let base = |pairs: &[(&str, &str)]| {
            let env = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            allowed_base(&env).map(|url| url.to_string())
        };
        let typesafe = "https://api.typesafe.ai/".to_string();
        assert_eq!(base(&[]), Ok(typesafe.clone()));
        assert_eq!(base(&[(BASE_URL_ENV, "  ")]), Ok(typesafe));
        assert_eq!(
            base(&[(LEGACY_BASE_URL_ENV, "http://127.0.0.1:9/x")]),
            Ok("http://127.0.0.1:9/x".to_string())
        );
        assert_eq!(
            base(&[
                (BASE_URL_ENV, "http://a/"),
                (LEGACY_BASE_URL_ENV, "http://b/")
            ]),
            Ok("http://a/".to_string())
        );
        assert_eq!(
            base(&[(PROVIDER_ENV, "openai"), (LEGACY_BASE_URL_ENV, "http://b/")]),
            Ok("https://api.openai.com/".to_string())
        );
        assert!(base(&[(PROVIDER_ENV, "other")]).is_err());
        assert!(base(&[(BASE_URL_ENV, "ftp://h/")]).is_err());
    }
}

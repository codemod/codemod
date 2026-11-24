//! The decision APIs over HTTP: TypeSafe's System One (`POST /v1/systemone`)
//! and OpenAI's Decisions API (`POST /v1/decisions`).
//!
//! Requests are retried on connection errors, timeouts and HTTP 408, 409,
//! 429 and 5xx, with jittered exponential backoff (500 ms doubling to 8 s),
//! or after `retry-after-ms` / `Retry-After` when the server sends one (up to
//! 60 s). At most `DECISION_CONCURRENCY` requests are in flight at once.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{json, Map, Value};

use super::config::{self, Env, Layers, ModelPreference, Provider, Settings};
use super::{
    DecisionAnswer, DecisionModel, DecisionQuestion, DecisionRequest, DecisionResponse, Limiter,
    TokenUsage, CANCELED,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryPolicy {
    pub max_retries: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(8),
        }
    }
}

const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
const ERROR_BODY_LIMIT: usize = 500;

/// The decision models a run may call, chosen per call from the call's
/// options, the workflow, the user config and the environment
/// ([`config::resolve`]).
pub struct HttpDecisionModels {
    layers: Layers,
    env: Env,
    /// Keys given for this run only (the CLI asked for one), by provider.
    keys: HashMap<Provider, String>,
    limiter: Limiter,
    retry: RetryPolicy,
}

impl HttpDecisionModels {
    pub fn new(layers: Layers, env: Env) -> Result<Self, String> {
        let limit = config::concurrency(&env)?;
        Ok(Self {
            layers,
            env,
            keys: HashMap::new(),
            limiter: Limiter::new(limit),
            retry: RetryPolicy::default(),
        })
    }

    /// Use `key` for `provider` when the environment has none.
    pub fn with_key(mut self, provider: Provider, key: String) -> Self {
        self.keys.insert(provider, key);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// The settings a call naming `call` gets, with the key given for the
    /// run when the environment has none.
    pub fn settings(&self, call: &ModelPreference) -> Result<Settings, String> {
        let mut settings = config::resolve(call, &self.layers, &self.env)?;
        if settings.api_key.is_none() {
            settings.api_key = self.keys.get(&settings.provider).cloned();
        }
        Ok(settings)
    }
}

impl DecisionModel for HttpDecisionModels {
    fn decide(
        &self,
        request: &DecisionRequest,
        cancel: &AtomicBool,
    ) -> Result<DecisionResponse, String> {
        let settings = self.settings(&ModelPreference {
            provider: request.provider.clone(),
            model: request.model.clone(),
        })?;
        let Some(key) = settings.api_key.clone() else {
            return Err(format!(
                "no API key for {} ({}): set {}",
                settings.provider.display_name(),
                settings.host(),
                settings.key_vars()
            ));
        };
        let body = encode(&settings, request)?;
        let _permit = self.limiter.acquire(cancel)?;
        let answer = post(&settings, &key, &body, request.timeout, self.retry, cancel)?;
        decode(&settings, request, &answer)
    }
}

/// The request body for `settings.provider`.
pub(crate) fn encode(settings: &Settings, request: &DecisionRequest) -> Result<Value, String> {
    match settings.provider {
        Provider::SystemOne => {
            let questions: Map<String, Value> = request
                .questions
                .iter()
                .map(|(id, question)| (id.clone(), systemone_question(question)))
                .collect();
            Ok(json!({ "model": settings.model, "state": request.state, "questions": questions }))
        }
        Provider::OpenAi => {
            if request.questions.len() > 400 {
                return Err("OpenAI Decisions take at most 400 questions per request".into());
            }
            let questions: Vec<Value> = request
                .questions
                .iter()
                .map(|(id, question)| openai_question(id, question))
                .collect();
            Ok(json!({ "model": settings.model, "input": request.state, "questions": questions }))
        }
    }
}

fn systemone_question(question: &DecisionQuestion) -> Value {
    match question {
        DecisionQuestion::YesNo {
            instructions,
            yes,
            no,
        } => {
            let mut q = json!({ "type": "noul", "instructions": instructions });
            if yes.is_some() || no.is_some() {
                q["criteria"] = json!({ "true": yes, "false": no });
            }
            q
        }
        DecisionQuestion::Choice {
            instructions,
            options,
        } => {
            let criteria: Map<String, Value> = options
                .iter()
                .map(|(name, description)| (name.clone(), json!(description)))
                .collect();
            json!({ "type": "choice", "instructions": instructions, "criteria": criteria })
        }
        DecisionQuestion::Score {
            instructions,
            levels,
        } => {
            let criteria: Vec<Value> = levels
                .iter()
                .map(|(label, description)| match description {
                    Some(description) => json!(format!("{label}: {description}")),
                    None => json!(label),
                })
                .collect();
            json!({ "type": "score", "instructions": instructions, "criteria": criteria })
        }
    }
}

fn openai_question(name: &str, question: &DecisionQuestion) -> Value {
    match question {
        DecisionQuestion::YesNo {
            instructions,
            yes,
            no,
        } => {
            let mut text = instructions.clone();
            if let Some(yes) = yes {
                text.push_str(&format!("\n\nAnswer yes when: {yes}"));
            }
            if let Some(no) = no {
                text.push_str(&format!("\n\nAnswer no when: {no}"));
            }
            json!({ "type": "predicate", "name": name, "instructions": text })
        }
        DecisionQuestion::Choice {
            instructions,
            options,
        } => {
            let choices: Vec<Value> = options
                .iter()
                .map(|(value, description)| {
                    json!({ "value": value, "description": description.as_deref().unwrap_or(value) })
                })
                .collect();
            json!({ "type": "choice", "name": name, "instructions": instructions, "choices": choices })
        }
        DecisionQuestion::Score {
            instructions,
            levels,
        } => {
            let levels: Vec<Value> = levels
                .iter()
                .map(|(label, description)| {
                    json!({ "label": label, "description": description.as_deref().unwrap_or(label) })
                })
                .collect();
            json!({ "type": "score", "name": name, "instructions": instructions, "levels": levels })
        }
    }
}

/// The response in the engine's shapes. Answer values are checked when the
/// typed answers are made.
pub(crate) fn decode(
    settings: &Settings,
    request: &DecisionRequest,
    body: &Value,
) -> Result<DecisionResponse, String> {
    let bad = |what: &str| {
        format!(
            "{} returned a response the engine cannot read: {what}",
            settings.provider.display_name()
        )
    };
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(&settings.model)
        .to_string();
    let usage = body.get("usage").cloned().unwrap_or(Value::Null);
    let count = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
    let usage = TokenUsage {
        input_tokens: count("input_tokens"),
        output_tokens: count("output_tokens"),
    };
    let mut answers = BTreeMap::new();
    match settings.provider {
        Provider::SystemOne => {
            let Some(got) = body.get("answers").and_then(Value::as_object) else {
                return Err(bad("answers must be an object"));
            };
            for (id, answer) in got {
                answers.insert(
                    id.clone(),
                    systemone_answer(answer).ok_or_else(|| bad(&format!("answer '{id}'")))?,
                );
            }
        }
        Provider::OpenAi => {
            let Some(got) = body.get("answers").and_then(Value::as_array) else {
                return Err(bad("answers must be an array"));
            };
            for (index, answer) in got.iter().enumerate() {
                // Answers come in question order; the name says whose.
                let name = answer
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| request.questions.get(index).map(|(id, _)| id.clone()))
                    .ok_or_else(|| bad("an answer has no name"))?;
                answers.insert(
                    name.clone(),
                    openai_answer(answer).ok_or_else(|| bad(&format!("answer '{name}'")))?,
                );
            }
        }
    }
    Ok(DecisionResponse {
        provider: settings.provider.as_str().to_string(),
        model,
        answers,
        usage,
    })
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn systemone_answer(answer: &Value) -> Option<DecisionAnswer> {
    let probabilities = || -> Option<BTreeMap<String, f64>> {
        answer
            .get("probabilities")?
            .as_object()?
            .iter()
            .map(|(k, v)| Some((k.clone(), v.as_f64()?)))
            .collect()
    };
    Some(match answer.get("type")?.as_str()? {
        "noul" => DecisionAnswer::YesNo {
            probability: number(answer, "noul")?,
        },
        "choice" => DecisionAnswer::Choice {
            choice: answer.get("choice")?.as_str()?.to_string(),
            probabilities: probabilities()?,
            confidence: number(answer, "confidence")?,
        },
        "score" => DecisionAnswer::Score {
            score: number(answer, "score")?,
            probabilities: probabilities()?
                .into_iter()
                .map(|(level, p)| Some((level.parse().ok()?, p)))
                .collect::<Option<_>>()?,
            confidence: number(answer, "confidence")?,
        },
        _ => return None,
    })
}

fn openai_answer(answer: &Value) -> Option<DecisionAnswer> {
    let entries = || answer.get("probabilities")?.as_array();
    Some(match answer.get("type")?.as_str()? {
        "refusal" => DecisionAnswer::Refused,
        "predicate" => DecisionAnswer::YesNo {
            probability: number(answer, "probability")?,
        },
        "choice" => DecisionAnswer::Choice {
            choice: answer.get("choice")?.as_str()?.to_string(),
            probabilities: entries()?
                .iter()
                .map(|e| {
                    Some((
                        e.get("value")?.as_str()?.to_string(),
                        number(e, "probability")?,
                    ))
                })
                .collect::<Option<_>>()?,
            confidence: number(answer, "confidence")?,
        },
        "score" => DecisionAnswer::Score {
            score: number(answer, "score")?,
            probabilities: entries()?
                .iter()
                .map(|e| {
                    Some((
                        usize::try_from(e.get("value")?.as_u64()?).ok()?,
                        number(e, "probability")?,
                    ))
                })
                .collect::<Option<_>>()?,
            confidence: number(answer, "confidence")?,
        },
        _ => return None,
    })
}

/// POST `body` and return the parsed 2xx response. Runs on a thread of its
/// own with its own runtime: callers may be inside one already.
fn post(
    settings: &Settings,
    key: &str,
    body: &Value,
    timeout: Duration,
    retry: RetryPolicy,
    cancel: &AtomicBool,
) -> Result<Value, String> {
    let body = body.to_string();
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?
                    .block_on(async {
                        tokio::select! {
                            answer = post_with_retries(settings, key, body, timeout, retry) => answer,
                            () = canceled(cancel) => Err(CANCELED.to_string()),
                        }
                    })
            })
            .join()
            .map_err(|_| "the decision request panicked".to_string())?
    })
}

async fn canceled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn post_with_retries(
    settings: &Settings,
    key: &str,
    body: String,
    timeout: Duration,
    retry: RetryPolicy,
) -> Result<Value, String> {
    let name = settings.provider.display_name();
    let client = reqwest::Client::builder()
        .user_agent(concat!("codemod/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let (header, value) = if settings.auth_header == "authorization" {
        ("authorization".to_string(), format!("Bearer {key}"))
    } else {
        (settings.auth_header.clone(), key.to_string())
    };
    let mut attempt = 0;
    loop {
        let sent = client
            .post(&settings.url)
            .header(header.as_str(), value.as_str())
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .timeout(timeout)
            .body(body.clone())
            .send()
            .await;
        let (failure, retryable, wait) = match sent {
            Ok(response) => {
                let status = response.status();
                let wait = retry_after(response.headers());
                let text = response.text().await.unwrap_or_default();
                if status.is_success() {
                    return serde_json::from_str(&text)
                        .map_err(|_| format!("{name} returned a body that is not JSON"));
                }
                let code = status.as_u16();
                let retryable = matches!(code, 408 | 409 | 429) || status.is_server_error();
                let detail: String = text.chars().take(ERROR_BODY_LIMIT).collect();
                let failure = if code == 401 || code == 403 {
                    format!(
                        "{name} refused the key ({code}): check {}",
                        settings.key_vars()
                    )
                } else if detail.trim().is_empty() {
                    format!("{name} responded {code}")
                } else {
                    format!("{name} responded {code}: {}", detail.trim())
                };
                (failure, retryable, wait)
            }
            Err(error) if error.is_timeout() => (
                format!("{name} did not answer within {}s", timeout.as_secs_f64()),
                true,
                None,
            ),
            Err(error) => (
                // Without the URL's query or the request: never the key.
                format!(
                    "could not reach {name} at {}: {}",
                    settings.host(),
                    error.without_url()
                ),
                true,
                None,
            ),
        };
        if !retryable || attempt >= retry.max_retries {
            return Err(failure);
        }
        tokio::time::sleep(wait.unwrap_or_else(|| backoff(retry, attempt))).await;
        attempt += 1;
    }
}

fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let delay = if let Some(ms) = text("retry-after-ms").and_then(|v| v.trim().parse::<f64>().ok())
    {
        Duration::try_from_secs_f64(ms / 1000.0).ok()?
    } else {
        let seconds = text("retry-after")?.trim().parse::<f64>().ok()?;
        Duration::try_from_secs_f64(seconds).ok()?
    };
    (delay <= MAX_RETRY_AFTER).then_some(delay)
}

fn backoff(retry: RetryPolicy, attempt: u32) -> Duration {
    let delay = retry
        .initial_delay
        .saturating_mul(2u32.saturating_pow(attempt))
        .min(retry.max_delay);
    // Up to a quarter less, so workers that failed together retry apart.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    delay.mul_f64(1.0 - 0.25 * (f64::from(nanos % 1000) / 1000.0))
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use super::*;

    /// A server that answers each request with the next of `responses`
    /// (status, extra headers, body), and keeps what it was sent.
    struct Fake {
        url: String,
        seen: Arc<Mutex<Vec<(String, String)>>>,
    }

    fn fake(responses: Vec<(u16, &'static str, String)>) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for (status, headers, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let mut request = vec![0; length];
                reader.read_exact(&mut request).unwrap();
                log.lock()
                    .unwrap()
                    .push((head, String::from_utf8(request).unwrap()));
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).unwrap();
            }
        });
        Fake { url, seen }
    }

    fn env(pairs: &[(&str, &str)]) -> Env {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn request(questions: Vec<(&str, DecisionQuestion)>) -> DecisionRequest {
        DecisionRequest {
            provider: None,
            model: None,
            state: "File: a.ts".into(),
            questions: questions
                .into_iter()
                .map(|(id, q)| (id.to_string(), q))
                .collect(),
            timeout: Duration::from_secs(5),
        }
    }

    fn yes_no() -> DecisionQuestion {
        DecisionQuestion::YesNo {
            instructions: "Handles errors?".into(),
            yes: Some("It catches them".into()),
            no: None,
        }
    }

    fn fast() -> RetryPolicy {
        RetryPolicy {
            max_retries: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
        }
    }

    #[test]
    fn system_one_gets_a_bearer_key_and_noul_questions_and_rate_limits_are_retried() {
        let server = fake(vec![
            (429, "retry-after-ms: 5\r\n", r#"{"detail":"slow down"}"#.into()),
            (
                200,
                "",
                json!({
                    "model": "jev-1.13.0",
                    "answers": {
                        "ok": { "type": "noul", "noul": 0.25 },
                        "kind": { "type": "choice", "choice": "b", "confidence": 0.7, "probabilities": { "a": 0.2, "b": 0.8 } },
                        "n": { "type": "score", "score": 1.5, "confidence": 0.4, "legend": {}, "probabilities": { "0": 0.1, "1": 0.3, "2": 0.6 } },
                    },
                    "usage": { "input_tokens": 120, "output_tokens": 12 },
                })
                .to_string(),
            ),
        ]);
        let models = HttpDecisionModels::new(
            Layers::default(),
            env(&[
                ("TYPESAFE_API_KEY", "secret-key"),
                ("TYPESAFE_BASE_URL", &server.url),
            ]),
        )
        .unwrap()
        .with_retry(fast());
        let response = models
            .decide(
                &request(vec![
                    ("ok", yes_no()),
                    (
                        "kind",
                        DecisionQuestion::Choice {
                            instructions: "Which?".into(),
                            options: vec![("a".into(), None), ("b".into(), Some("Bee".into()))],
                        },
                    ),
                    (
                        "n",
                        DecisionQuestion::Score {
                            instructions: "How many?".into(),
                            levels: vec![
                                ("1".into(), None),
                                ("2".into(), Some("two".into())),
                                ("3".into(), None),
                            ],
                        },
                    ),
                ]),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(response.provider, "systemone");
        assert_eq!(
            response.usage,
            TokenUsage {
                input_tokens: 120,
                output_tokens: 12
            }
        );
        assert_eq!(
            response.answers["ok"],
            DecisionAnswer::YesNo { probability: 0.25 }
        );
        let DecisionAnswer::Score { probabilities, .. } = &response.answers["n"] else {
            panic!()
        };
        assert_eq!(probabilities[&2], 0.6);

        let seen = server.seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "the 429 was retried");
        let (head, body) = &seen[1];
        assert!(head.starts_with("POST /v1/systemone "), "{head}");
        assert!(head
            .to_ascii_lowercase()
            .contains("authorization: bearer secret-key"));
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["state"], "File: a.ts");
        assert_eq!(
            body["questions"]["ok"],
            json!({ "type": "noul", "instructions": "Handles errors?", "criteria": { "true": "It catches them", "false": null } })
        );
        assert_eq!(
            body["questions"]["kind"]["criteria"],
            json!({ "a": null, "b": "Bee" })
        );
        assert_eq!(
            body["questions"]["n"]["criteria"],
            json!(["1", "2: two", "3"])
        );
    }

    #[test]
    fn openai_gets_named_questions_and_answers_come_back_by_name() {
        let server = fake(vec![(
            200,
            "",
            json!({
                "model": "gpt-6-luna",
                "answers": [
                    { "type": "predicate", "name": "ok", "probability": 0.9 },
                    { "type": "refusal", "name": "kind" },
                    { "type": "score", "name": "n", "score": 0.2, "confidence": 0.8, "probabilities": [{ "label": "1", "value": 0, "probability": 0.8 }, { "label": "2", "value": 1, "probability": 0.2 }] },
                ],
                "usage": { "input_tokens": 42, "output_tokens": 0, "total_tokens": 42 },
            })
            .to_string(),
        )]);
        let models = HttpDecisionModels::new(
            Layers::default(),
            env(&[
                ("DECISION_PROVIDER", "openai"),
                ("OPENAI_API_KEY", "sk-test"),
                ("OPENAI_BASE_URL", &server.url),
            ]),
        )
        .unwrap();
        let response = models
            .decide(
                &request(vec![
                    ("ok", yes_no()),
                    (
                        "kind",
                        DecisionQuestion::Choice {
                            instructions: "Which?".into(),
                            options: vec![("a".into(), None), ("b".into(), None)],
                        },
                    ),
                    (
                        "n",
                        DecisionQuestion::Score {
                            instructions: "How many?".into(),
                            levels: vec![("1".into(), None), ("2".into(), None)],
                        },
                    ),
                ]),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(response.answers["kind"], DecisionAnswer::Refused);
        assert_eq!(
            response.answers["ok"],
            DecisionAnswer::YesNo { probability: 0.9 }
        );
        assert_eq!(response.usage.input_tokens, 42);
        let seen = server.seen.lock().unwrap();
        let (head, body) = &seen[0];
        assert!(head.starts_with("POST /v1/decisions "), "{head}");
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["model"], "gpt-6-luna");
        assert_eq!(body["input"], "File: a.ts");
        assert_eq!(
            body["questions"][0],
            json!({ "type": "predicate", "name": "ok", "instructions": "Handles errors?\n\nAnswer yes when: It catches them" })
        );
        assert_eq!(
            body["questions"][1]["choices"],
            json!([{ "value": "a", "description": "a" }, { "value": "b", "description": "b" }])
        );
        assert_eq!(
            body["questions"][2]["levels"][1],
            json!({ "label": "2", "description": "2" })
        );
    }

    #[test]
    fn a_missing_key_names_the_variable_and_a_refused_key_never_shows_it() {
        let models = HttpDecisionModels::new(Layers::default(), env(&[])).unwrap();
        let error = models
            .decide(&request(vec![("ok", yes_no())]), &AtomicBool::new(false))
            .unwrap_err();
        assert!(
            error.contains("set TYPESAFE_API_KEY (or DECISION_API_KEY)"),
            "{error}"
        );

        let server = fake(vec![(401, "", r#"{"detail":"bad key"}"#.into())]);
        let models = HttpDecisionModels::new(
            Layers::default(),
            env(&[
                ("TYPESAFE_API_KEY", "secret-key"),
                ("TYPESAFE_BASE_URL", &server.url),
            ]),
        )
        .unwrap();
        let error = models
            .decide(&request(vec![("ok", yes_no())]), &AtomicBool::new(false))
            .unwrap_err();
        assert!(error.contains("refused the key (401)"), "{error}");
        assert!(!error.contains("secret-key"), "{error}");
    }

    #[test]
    fn server_errors_stop_after_the_retries() {
        let server = fake(vec![
            (500, "", "{}".into()),
            (503, "", "{}".into()),
            (502, "", "upstream".into()),
        ]);
        let models = HttpDecisionModels::new(
            Layers::default(),
            env(&[
                ("TYPESAFE_API_KEY", "k"),
                ("TYPESAFE_BASE_URL", &server.url),
            ]),
        )
        .unwrap()
        .with_retry(fast());
        let error = models
            .decide(&request(vec![("ok", yes_no())]), &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(error, "System One responded 502: upstream");
        assert_eq!(server.seen.lock().unwrap().len(), 3);
    }

    #[test]
    fn retry_after_is_read_in_milliseconds_or_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "2".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(2)));
        headers.insert("retry-after-ms", "150".parse().unwrap());
        assert_eq!(retry_after(&headers), Some(Duration::from_millis(150)));
        headers.insert("retry-after-ms", "600000".parse().unwrap());
        assert_eq!(retry_after(&headers), None);
        assert!(backoff(RetryPolicy::default(), 10) <= Duration::from_secs(8));
    }
}

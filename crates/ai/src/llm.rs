use rig::client::CompletionClient;
use rig::completion::{AssistantContent, CompletionModel};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub provider: String,
    pub system_prompt: Option<String>,
    pub prompt: String,
    pub output_schema: Option<Value>,
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub uncached_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateResponse {
    pub output: String,
    pub provider: String,
    pub model: String,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Error)]
pub enum GenerateError {
    #[error("Unsupported LLM provider: {0}")]
    UnsupportedProvider(String),
    #[error("Failed to configure LLM provider: {0}")]
    Configuration(String),
    #[error("LLM request failed: {0}")]
    Request(String),
    #[error("LLM response did not contain text")]
    MissingText,
    #[error("Invalid output schema: {0}")]
    InvalidSchema(String),
}

fn response_text(
    choices: impl IntoIterator<Item = AssistantContent>,
) -> Result<String, GenerateError> {
    let output = choices
        .into_iter()
        .filter_map(|choice| match choice {
            AssistantContent::Text(text) => Some(text.text),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    if output.is_empty() {
        Err(GenerateError::MissingText)
    } else {
        Ok(output)
    }
}

fn output_schema(value: Option<Value>) -> Result<Option<schemars::Schema>, GenerateError> {
    value
        .map(|value| {
            value
                .try_into()
                .map_err(|error| GenerateError::InvalidSchema(format!("{error:?}")))
        })
        .transpose()
}

async fn generate_openai(request: GenerateRequest) -> Result<GenerateResponse, GenerateError> {
    use rig::providers::openai;

    let client: openai::Client = openai::Client::builder()
        .api_key(request.api_key)
        .base_url(request.endpoint)
        .build()
        .map_err(|error| GenerateError::Configuration(error.to_string()))?;
    let model = client.completion_model(request.model.clone());
    let mut completion = model
        .completion_request(request.prompt)
        .max_tokens_opt(request.max_tokens)
        .output_schema_opt(output_schema(request.output_schema)?);
    if let Some(system_prompt) = request.system_prompt {
        completion = completion.preamble(system_prompt);
    }
    let response = completion
        .send()
        .await
        .map_err(|error| GenerateError::Request(error.to_string()))?;
    let output = response_text(response.choice)?;
    let raw = response.raw_response;
    let model = raw.model;
    let usage = raw.usage.map(|usage| {
        let cache_read_input_tokens = usage
            .input_tokens_details
            .as_ref()
            .map(|details| details.cached_tokens);
        TokenUsage {
            uncached_input_tokens: Some(
                usage
                    .input_tokens
                    .saturating_sub(cache_read_input_tokens.unwrap_or(0)),
            ),
            cache_read_input_tokens,
            cache_write_input_tokens: None,
            output_tokens: Some(usage.output_tokens),
            reasoning_tokens: Some(usage.output_tokens_details.reasoning_tokens),
            total_tokens: Some(usage.total_tokens),
        }
    });

    Ok(GenerateResponse {
        output,
        provider: "openai".to_string(),
        model,
        usage,
    })
}

async fn generate_anthropic(request: GenerateRequest) -> Result<GenerateResponse, GenerateError> {
    use rig::providers::anthropic;

    let client: anthropic::Client = anthropic::Client::builder()
        .api_key(request.api_key)
        .base_url(request.endpoint)
        .build()
        .map_err(|error| GenerateError::Configuration(error.to_string()))?;
    let model = client.completion_model(request.model.clone());
    let mut completion = model
        .completion_request(request.prompt)
        .max_tokens(request.max_tokens.unwrap_or(4096))
        .output_schema_opt(output_schema(request.output_schema)?);
    if let Some(system_prompt) = request.system_prompt {
        completion = completion.preamble(system_prompt);
    }
    let response = completion
        .send()
        .await
        .map_err(|error| GenerateError::Request(error.to_string()))?;
    let output = response_text(response.choice)?;
    let raw = response.raw_response;
    let model = raw.model;
    let usage = raw.usage;
    let cache_read_input_tokens = usage.cache_read_input_tokens;
    let cache_write_input_tokens = usage.cache_creation_input_tokens;
    let total_tokens = usage.input_tokens
        + cache_read_input_tokens.unwrap_or(0)
        + cache_write_input_tokens.unwrap_or(0)
        + usage.output_tokens;

    Ok(GenerateResponse {
        output,
        provider: "anthropic".to_string(),
        model,
        usage: Some(TokenUsage {
            uncached_input_tokens: Some(usage.input_tokens),
            cache_read_input_tokens,
            cache_write_input_tokens,
            output_tokens: Some(usage.output_tokens),
            reasoning_tokens: None,
            total_tokens: Some(total_tokens),
        }),
    })
}

pub async fn generate(request: GenerateRequest) -> Result<GenerateResponse, GenerateError> {
    match request.provider.trim().to_ascii_lowercase().as_str() {
        "openai" | "openai_compatible" => generate_openai(request).await,
        "anthropic" => generate_anthropic(request).await,
        provider => Err(GenerateError::UnsupportedProvider(provider.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    async fn serve_once_with_status(status: &str, response: Value) -> (String, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock server should bind");
        let address = listener
            .local_addr()
            .expect("mock server should have an address");
        let status = status.to_string();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("request should connect");
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            let header_end = loop {
                let read = stream
                    .read(&mut chunk)
                    .await
                    .expect("request should be readable");
                assert!(read > 0, "request ended before its headers");
                request.extend_from_slice(&chunk[..read]);
                if let Some(position) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().expect("valid content length"))
                    })
                })
                .unwrap_or_default();
            while request.len() < header_end + content_length {
                let read = stream
                    .read(&mut chunk)
                    .await
                    .expect("request body should be readable");
                assert!(read > 0, "request ended before its body");
                request.extend_from_slice(&chunk[..read]);
            }

            let response = serde_json::to_vec(&response).expect("response should serialize");
            let headers = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                response.len()
            );
            stream
                .write_all(headers.as_bytes())
                .await
                .expect("response headers should write");
            stream
                .write_all(&response)
                .await
                .expect("response body should write");
            stream.shutdown().await.expect("response should finish");

            String::from_utf8(request).expect("request should be UTF-8")
        });
        (format!("http://{address}"), handle)
    }

    async fn serve_once(response: Value) -> (String, JoinHandle<String>) {
        serve_once_with_status("200 OK", response).await
    }

    fn request(provider: &str, endpoint: String) -> GenerateRequest {
        GenerateRequest {
            endpoint,
            api_key: "test-key".to_string(),
            model: "requested-model".to_string(),
            provider: provider.to_string(),
            system_prompt: Some("Return JSON".to_string()),
            prompt: "Generate an identifier".to_string(),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "identifier": { "type": "string" }
                },
                "required": ["identifier"],
                "additionalProperties": false
            })),
            max_tokens: Some(128),
        }
    }

    #[tokio::test]
    async fn captures_openai_responses_usage_without_javascript_telemetry() {
        let (endpoint, server) = serve_once(json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "error": null,
            "incomplete_details": null,
            "instructions": null,
            "max_output_tokens": 128,
            "model": "gpt-test",
            "usage": {
                "input_tokens": 100,
                "input_tokens_details": { "cached_tokens": 40 },
                "output_tokens": 20,
                "output_tokens_details": { "reasoning_tokens": 5 },
                "total_tokens": 120
            },
            "output": [{
                "type": "message",
                "id": "msg_test",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "{\"identifier\":\"welcomeTitle\"}"
                }]
            }],
            "tools": []
        }))
        .await;

        let response = generate(request("openai", endpoint))
            .await
            .expect("OpenAI response should succeed");
        let captured_request = server.await.expect("mock server should finish");

        assert!(captured_request.starts_with("POST /responses "));
        assert_eq!(response.output, "{\"identifier\":\"welcomeTitle\"}");
        assert_eq!(response.provider, "openai");
        assert_eq!(response.model, "gpt-test");
        assert_eq!(
            response.usage,
            Some(TokenUsage {
                uncached_input_tokens: Some(60),
                cache_read_input_tokens: Some(40),
                cache_write_input_tokens: None,
                output_tokens: Some(20),
                reasoning_tokens: Some(5),
                total_tokens: Some(120),
            })
        );
    }

    #[tokio::test]
    async fn captures_anthropic_cache_usage_without_javascript_telemetry() {
        let (endpoint, server) = serve_once(json!({
            "id": "msg_test",
            "type": "message",
            "role": "assistant",
            "model": "claude-test",
            "content": [{
                "type": "text",
                "text": "{\"identifier\":\"welcomeTitle\"}"
            }],
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {
                "input_tokens": 10,
                "cache_read_input_tokens": 4,
                "cache_creation_input_tokens": 2,
                "output_tokens": 3
            }
        }))
        .await;

        let response = generate(request("anthropic", endpoint))
            .await
            .expect("Anthropic response should succeed");
        let captured_request = server.await.expect("mock server should finish");

        assert!(captured_request.starts_with("POST /v1/messages "));
        assert_eq!(response.output, "{\"identifier\":\"welcomeTitle\"}");
        assert_eq!(response.provider, "anthropic");
        assert_eq!(response.model, "claude-test");
        assert_eq!(
            response.usage,
            Some(TokenUsage {
                uncached_input_tokens: Some(10),
                cache_read_input_tokens: Some(4),
                cache_write_input_tokens: Some(2),
                output_tokens: Some(3),
                reasoning_tokens: None,
                total_tokens: Some(19),
            })
        );
    }

    #[tokio::test]
    async fn preserves_missing_openai_usage_as_unavailable() {
        let (endpoint, server) = serve_once(json!({
            "id": "resp_test",
            "object": "response",
            "created_at": 1,
            "status": "completed",
            "error": null,
            "incomplete_details": null,
            "instructions": null,
            "max_output_tokens": null,
            "model": "gpt-test",
            "usage": null,
            "output": [{
                "type": "message",
                "id": "msg_test",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": "welcomeTitle"
                }]
            }],
            "tools": []
        }))
        .await;

        let response = generate(request("openai", endpoint))
            .await
            .expect("OpenAI response without usage should still succeed");
        server.await.expect("mock server should finish");

        assert_eq!(response.output, "welcomeTitle");
        assert_eq!(response.usage, None);
    }

    #[tokio::test]
    async fn returns_provider_errors_without_inventing_usage() {
        let (endpoint, server) = serve_once_with_status(
            "429 Too Many Requests",
            json!({
                "error": {
                    "message": "rate limited",
                    "type": "rate_limit_error"
                }
            }),
        )
        .await;

        let error = generate(request("openai", endpoint))
            .await
            .expect_err("provider failure should fail generation");
        server.await.expect("mock server should finish");

        assert!(matches!(error, GenerateError::Request(_)));
    }
}

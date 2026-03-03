use std::path::PathBuf;

use rig::completion::Prompt;
use thiserror::Error;

use crate::prompt::{build_system_context, build_system_prompt_with_context, build_user_message};
use crate::tools::registry::{create_cli_tool_server_handle, get_default_cli_tools};

const TASK_DONE_REASON_PREFIX: &str = "__task_done__:";

pub struct ExecuteAiStepConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_steps: Option<usize>,
    pub enable_lakeview: Option<bool>,
    pub prompt: String,
    pub working_dir: PathBuf,
    pub llm_protocol: String,
}

#[derive(Debug, Clone, Default)]
pub struct ExecuteAiStepResult {
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Error)]
pub enum ExecuteAiStepError {
    #[error("AI execution failed: {0}")]
    Execution(String),
}

#[derive(Debug, Clone)]
struct NormalizedAiExecutionConfig {
    endpoint: String,
    api_key: String,
    model: String,
    system_prompt: Option<String>,
    max_steps: usize,
    prompt: String,
    working_dir: PathBuf,
    llm_protocol: LlmProtocol,
    tools: Vec<String>,
}

impl From<ExecuteAiStepConfig> for NormalizedAiExecutionConfig {
    fn from(value: ExecuteAiStepConfig) -> Self {
        let tools = get_default_cli_tools();

        Self {
            endpoint: value.endpoint,
            api_key: value.api_key,
            model: value.model,
            system_prompt: value.system_prompt,
            max_steps: value.max_steps.unwrap_or(30),
            prompt: value.prompt,
            working_dir: value.working_dir,
            llm_protocol: LlmProtocol::from_raw(&value.llm_protocol),
            tools,
        }
    }
}

#[derive(Debug, Clone)]
enum LlmProtocol {
    OpenAICompat,
    Anthropic,
    GoogleAi,
    AzureOpenAi,
    Custom(String),
}

impl LlmProtocol {
    fn from_raw(raw: &str) -> Self {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "openai" => Self::OpenAICompat,
            "anthropic" => Self::Anthropic,
            "google_ai" => Self::GoogleAi,
            "azure_openai" => Self::AzureOpenAi,
            _ => Self::Custom(raw.trim().to_string()),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct TaskDoneTerminationHook;

impl<M> rig::agent::PromptHook<M> for TaskDoneTerminationHook
where
    M: rig::completion::CompletionModel,
{
    fn on_tool_result(
        &self,
        tool_name: &str,
        tool_call_id: Option<String>,
        internal_call_id: &str,
        args: &str,
        result: &str,
    ) -> impl std::future::Future<Output = rig::agent::HookAction> + rig::wasm_compat::WasmCompatSend
    {
        let _ = (tool_call_id, internal_call_id, args);
        let should_terminate = tool_name == "task_done";
        let result = result.to_string();

        async move {
            if should_terminate {
                rig::agent::HookAction::terminate(format!("{TASK_DONE_REASON_PREFIX}{result}"))
            } else {
                rig::agent::HookAction::cont()
            }
        }
    }
}

async fn resolve_tool_server_handle(
    tool_names: &[String],
) -> Result<rig::tool::server::ToolServerHandle, ExecuteAiStepError> {
    create_cli_tool_server_handle(tool_names)
        .await
        .map_err(|e| ExecuteAiStepError::Execution(format!("Failed to initialize tools: {}", e)))
}

fn build_preamble(config: &NormalizedAiExecutionConfig, available_tools: &[String]) -> String {
    let base_prompt = if let Some(custom_prompt) = &config.system_prompt {
        let system_context = build_system_context();
        format!("{}\n\n[System Context]:\n{}", custom_prompt, system_context)
    } else {
        build_system_prompt_with_context(&config.working_dir)
    };

    format!(
        "{}\n\nAvailable tools: {}",
        base_prompt,
        available_tools.join(", ")
    )
}

fn handle_prompt_response(
    result: Result<String, rig::completion::PromptError>,
) -> Result<String, ExecuteAiStepError> {
    match result {
        Ok(text) => Ok(text),
        Err(rig::completion::PromptError::PromptCancelled { reason, .. }) => reason
            .strip_prefix(TASK_DONE_REASON_PREFIX)
            .map(std::string::ToString::to_string)
            .ok_or(ExecuteAiStepError::Execution(reason)),
        Err(error) => Err(ExecuteAiStepError::Execution(error.to_string())),
    }
}

async fn prompt_with_client<C>(
    client: &C,
    config: &NormalizedAiExecutionConfig,
    preamble: &str,
    task_message: &str,
) -> Result<String, ExecuteAiStepError>
where
    C: rig::client::CompletionClient,
{
    let tool_server_handle = resolve_tool_server_handle(&config.tools).await?;
    let agent = client
        .agent(config.model.clone())
        .max_tokens(200_000)
        .temperature(0.7)
        .preamble(preamble)
        .hook(TaskDoneTerminationHook)
        .tool_server_handle(tool_server_handle)
        .build();

    let result = agent
        .prompt(task_message.to_string())
        .max_turns(config.max_steps)
        .with_tool_concurrency(1)
        .await;

    handle_prompt_response(result)
}

fn as_execution_error(error: impl std::fmt::Display) -> ExecuteAiStepError {
    ExecuteAiStepError::Execution(error.to_string())
}

async fn build_client_and_prompt<C, E, F>(
    build_client: F,
    config: &NormalizedAiExecutionConfig,
    preamble: &str,
    task_message: &str,
) -> Result<String, ExecuteAiStepError>
where
    C: rig::client::CompletionClient,
    E: std::fmt::Display,
    F: FnOnce() -> Result<C, E>,
{
    let client = build_client().map_err(as_execution_error)?;
    prompt_with_client(&client, config, preamble, task_message).await
}

async fn run_with_provider(
    config: &NormalizedAiExecutionConfig,
    preamble: &str,
    task_message: &str,
) -> Result<String, ExecuteAiStepError> {
    use rig::providers::{anthropic, azure, gemini, openai};

    match &config.llm_protocol {
        LlmProtocol::OpenAICompat => {
            build_client_and_prompt(
                || -> Result<openai::Client, rig::http_client::Error> {
                    openai::Client::builder()
                        .api_key(config.api_key.clone())
                        .base_url(config.endpoint.clone())
                        .build()
                },
                config,
                preamble,
                task_message,
            )
            .await
        }
        LlmProtocol::Anthropic => {
            build_client_and_prompt(
                || -> Result<anthropic::Client, rig::http_client::Error> {
                    anthropic::Client::builder()
                        .api_key(config.api_key.clone())
                        .base_url(config.endpoint.clone())
                        .build()
                },
                config,
                preamble,
                task_message,
            )
            .await
        }
        LlmProtocol::GoogleAi => {
            build_client_and_prompt(
                || -> Result<gemini::Client, rig::http_client::Error> {
                    gemini::Client::builder()
                        .api_key(config.api_key.clone())
                        .base_url(config.endpoint.clone())
                        .build()
                },
                config,
                preamble,
                task_message,
            )
            .await
        }
        LlmProtocol::AzureOpenAi => {
            build_client_and_prompt(
                || -> Result<azure::Client, rig::http_client::Error> {
                    azure::Client::builder()
                        .api_key(azure::AzureOpenAIAuth::ApiKey(config.api_key.clone()))
                        .azure_endpoint(config.endpoint.clone())
                        .build()
                },
                config,
                preamble,
                task_message,
            )
            .await
        }
        LlmProtocol::Custom(protocol) => Err(ExecuteAiStepError::Execution(format!(
            "Unsupported llm_protocol: {}",
            protocol
        ))),
    }
}
pub async fn execute_ai_step(
    config: ExecuteAiStepConfig,
) -> Result<ExecuteAiStepResult, ExecuteAiStepError> {
    let normalized = NormalizedAiExecutionConfig::from(config);
    let preamble = build_preamble(&normalized, &normalized.tools);
    let task_message = build_user_message(&normalized.prompt);

    let response = run_with_provider(&normalized, &preamble, &task_message).await?;

    Ok(ExecuteAiStepResult {
        data: Some(serde_json::Value::String(response)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_mapping() {
        assert!(matches!(
            LlmProtocol::from_raw("openai"),
            LlmProtocol::OpenAICompat
        ));
        assert!(matches!(
            LlmProtocol::from_raw("anthropic"),
            LlmProtocol::Anthropic
        ));
        assert!(matches!(
            LlmProtocol::from_raw("google_ai"),
            LlmProtocol::GoogleAi
        ));
        assert!(matches!(
            LlmProtocol::from_raw("azure_openai"),
            LlmProtocol::AzureOpenAi
        ));
        assert!(matches!(
            LlmProtocol::from_raw("custom-provider"),
            LlmProtocol::Custom(name) if name == "custom-provider"
        ));
    }

    #[test]
    fn test_custom_system_prompt_includes_system_context() {
        let config = NormalizedAiExecutionConfig {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            model: "gpt-4o".to_string(),
            system_prompt: Some("Custom prompt".to_string()),
            max_steps: 30,
            prompt: "Test".to_string(),
            working_dir: PathBuf::from("/tmp/project"),
            llm_protocol: LlmProtocol::OpenAICompat,
            tools: get_default_cli_tools(),
        };

        let preamble = build_preamble(&config, &config.tools);
        assert!(preamble.contains("Custom prompt"));
        assert!(preamble.contains("[System Context]:"));
        assert!(preamble.contains("Available tools:"));
    }

    #[test]
    fn test_default_tools_applied_when_unspecified() {
        let normalized = NormalizedAiExecutionConfig::from(ExecuteAiStepConfig {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            model: "gpt-4o".to_string(),
            system_prompt: None,
            max_steps: None,
            enable_lakeview: None,
            prompt: "Test".to_string(),
            working_dir: PathBuf::from("/tmp/project"),
            llm_protocol: "openai".to_string(),
        });

        assert_eq!(normalized.tools, get_default_cli_tools());
    }

    #[test]
    fn test_task_done_prompt_cancelled_maps_to_success_response() {
        let result = handle_prompt_response(Err(rig::completion::PromptError::PromptCancelled {
            chat_history: Box::new(Vec::new()),
            reason: format!("{TASK_DONE_REASON_PREFIX}Summary: done"),
        }))
        .unwrap();

        assert_eq!(result, "Summary: done");
    }

    #[test]
    fn test_non_task_done_prompt_cancelled_maps_to_error() {
        let error = handle_prompt_response(Err(rig::completion::PromptError::PromptCancelled {
            chat_history: Box::new(Vec::new()),
            reason: "cancelled for another reason".to_string(),
        }))
        .unwrap_err();

        assert!(matches!(error, ExecuteAiStepError::Execution(_)));
        assert_eq!(
            error.to_string(),
            "AI execution failed: cancelled for another reason"
        );
    }

    #[tokio::test]
    async fn test_unknown_protocol_fails_fast() {
        let config = NormalizedAiExecutionConfig {
            endpoint: "https://example.com/v1".to_string(),
            api_key: "test-key".to_string(),
            model: "test-model".to_string(),
            system_prompt: None,
            max_steps: 1,
            prompt: "Test".to_string(),
            working_dir: PathBuf::from("/tmp/project"),
            llm_protocol: LlmProtocol::Custom("custom-provider".to_string()),
            tools: get_default_cli_tools(),
        };

        let error = run_with_provider(&config, "preamble", "task")
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "AI execution failed: Unsupported llm_protocol: custom-provider"
        );
    }
}

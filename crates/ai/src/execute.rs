use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use rig::client::embeddings::EmbeddingsClient;
use rig::completion::{Message, Prompt, PromptError, Usage};
use thiserror::Error;

use crate::memory::controller::{
    compact_history_for_retry, is_context_limit_error_text, is_proactive_cancel_reason,
    maybe_proactive_budget, proactive_cancel_reason, CompactionResult, CompactionStats,
    MemoryTrigger, TokenUsageSnapshot, MAX_COMPACTION_ATTEMPTS, SOFT_CONTEXT_CHAR_BUDGET,
    SOFT_CONTEXT_TOKEN_BUDGET,
};
use crate::memory::history::estimate_context_chars;
use crate::memory::semantic::{build_dynamic_context_index, DynamicContextIndex, SemanticDocument};
use crate::prompt::{build_system_context, build_system_prompt_with_context, build_user_message};
use crate::tools::registry::{create_cli_tool_server_handle, get_default_cli_tools};

const TASK_DONE_REASON_PREFIX: &str = "__task_done__:";
const DYNAMIC_CONTEXT_SAMPLE_DOCS: usize = 6;

pub struct ExecuteAiStepConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_steps: Option<usize>,
    pub tools: Option<Vec<String>>,
    pub prompt: String,
    pub working_dir: PathBuf,
    pub llm_protocol: String,
}

#[derive(Debug, Clone, Default)]
pub struct ExecuteAiStepResult {
    pub data: Option<serde_json::Value>,
    pub compaction_events: Vec<CompactionEventDiagnostics>,
}

#[derive(Debug, Error)]
pub enum ExecuteAiStepError {
    #[error("AI execution failed: {0}")]
    Execution(String),
    #[error("AI execution failed: Memory exhaustion: unable to compact context within limit. {diagnostics}")]
    MemoryExhaustion {
        diagnostics: MemoryExhaustionDiagnostics,
    },
}

impl ExecuteAiStepError {
    pub fn memory_exhaustion_diagnostics(&self) -> Option<&MemoryExhaustionDiagnostics> {
        match self {
            Self::MemoryExhaustion { diagnostics } => Some(diagnostics),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionEventDiagnostics {
    pub attempt: usize,
    pub trigger: String,
    pub before_chars: usize,
    pub after_chars: usize,
    pub archived_docs: usize,
    pub retrieved_docs: usize,
}

impl CompactionEventDiagnostics {
    fn from_stats(stats: &CompactionStats) -> Self {
        Self {
            attempt: stats.attempt,
            trigger: format!("{:?}", stats.trigger),
            before_chars: stats.before_chars,
            after_chars: stats.after_chars,
            archived_docs: stats.archived_docs,
            retrieved_docs: stats.retrieved_docs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryExhaustionDiagnostics {
    pub attempts: usize,
    pub trigger: Option<String>,
    pub before_chars: Option<usize>,
    pub after_chars: Option<usize>,
    pub archived_docs: Option<usize>,
    pub retrieved_docs: Option<usize>,
    pub soft_char_budget: usize,
    pub soft_token_budget: u64,
}

impl std::fmt::Display for MemoryExhaustionDiagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let (
            Some(trigger),
            Some(before_chars),
            Some(after_chars),
            Some(archived_docs),
            Some(retrieved_docs),
        ) = (
            &self.trigger,
            self.before_chars,
            self.after_chars,
            self.archived_docs,
            self.retrieved_docs,
        ) {
            return write!(
                f,
                "attempts={}, trigger={}, before_chars={}, after_chars={}, archived_docs={}, retrieved_docs={}, soft_char_budget={}, soft_token_budget={}",
                self.attempts,
                trigger,
                before_chars,
                after_chars,
                archived_docs,
                retrieved_docs,
                self.soft_char_budget,
                self.soft_token_budget
            );
        }

        write!(
            f,
            "attempts={}, soft_char_budget={}, soft_token_budget={}",
            self.attempts, self.soft_char_budget, self.soft_token_budget
        )
    }
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
        let tools = value.tools.unwrap_or_else(get_default_cli_tools);

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
struct TokenBudgetTracker {
    snapshot: Arc<RwLock<Option<TokenUsageSnapshot>>>,
}

impl TokenBudgetTracker {
    fn snapshot(&self) -> Option<TokenUsageSnapshot> {
        self.snapshot
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().cloned())
    }

    fn update(&self, usage: Usage, prompt: &Message, history: &[Message]) {
        if usage.total_tokens == 0 {
            return;
        }

        let context_chars = estimate_context_chars(prompt, history);
        if context_chars == 0 {
            return;
        }

        if let Ok(mut guard) = self.snapshot.write() {
            *guard = Some(TokenUsageSnapshot {
                total_tokens: usage.total_tokens,
                context_chars,
            });
        }
    }
}

#[derive(Debug, Clone, Default)]
struct TaskDoneAndMemoryHook {
    budget_tracker: TokenBudgetTracker,
}

impl<M> rig::agent::PromptHook<M> for TaskDoneAndMemoryHook
where
    M: rig::completion::CompletionModel,
{
    fn on_completion_call(
        &self,
        prompt: &Message,
        history: &[Message],
    ) -> impl std::future::Future<Output = rig::agent::HookAction> + rig::wasm_compat::WasmCompatSend
    {
        let budget = self.budget_tracker.snapshot();
        let proactive = maybe_proactive_budget(prompt, history, budget.as_ref());

        async move {
            if let Some((chars, estimated_tokens)) = proactive {
                rig::agent::HookAction::terminate(proactive_cancel_reason(chars, estimated_tokens))
            } else {
                rig::agent::HookAction::cont()
            }
        }
    }

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

fn task_done_output(reason: &str) -> Option<String> {
    reason
        .strip_prefix(TASK_DONE_REASON_PREFIX)
        .map(std::string::ToString::to_string)
}

fn build_memory_exhaustion_error(
    attempts: usize,
    stats: Option<&CompactionStats>,
) -> ExecuteAiStepError {
    let diagnostics = if let Some(stats) = stats {
        MemoryExhaustionDiagnostics {
            attempts,
            trigger: Some(format!("{:?}", stats.trigger)),
            before_chars: Some(stats.before_chars),
            after_chars: Some(stats.after_chars),
            archived_docs: Some(stats.archived_docs),
            retrieved_docs: Some(stats.retrieved_docs),
            soft_char_budget: SOFT_CONTEXT_CHAR_BUDGET,
            soft_token_budget: SOFT_CONTEXT_TOKEN_BUDGET,
        }
    } else {
        MemoryExhaustionDiagnostics {
            attempts,
            trigger: None,
            before_chars: None,
            after_chars: None,
            archived_docs: None,
            retrieved_docs: None,
            soft_char_budget: SOFT_CONTEXT_CHAR_BUDGET,
            soft_token_budget: SOFT_CONTEXT_TOKEN_BUDGET,
        }
    };

    ExecuteAiStepError::MemoryExhaustion { diagnostics }
}

async fn compact_and_retry<C>(
    client: &C,
    config: &NormalizedAiExecutionConfig,
    prompt_message: &Message,
    source_history: &[Message],
    compaction_attempts: usize,
    last_compaction_stats: Option<&CompactionStats>,
    trigger: MemoryTrigger,
) -> Result<CompactionResult, ExecuteAiStepError>
where
    C: rig::client::CompletionClient,
{
    if compaction_attempts >= MAX_COMPACTION_ATTEMPTS {
        return Err(build_memory_exhaustion_error(
            compaction_attempts,
            last_compaction_stats,
        ));
    }

    compact_history_for_retry(
        client,
        &config.model,
        &config.prompt,
        prompt_message,
        source_history,
        compaction_attempts,
        trigger,
    )
    .await
    .map_err(|e| {
        ExecuteAiStepError::Execution(format!("Failed to compact memory for retry: {}", e))
    })
}

fn apply_compaction_result(
    compaction: CompactionResult,
    chat_history: &mut Vec<Message>,
    semantic_docs: &mut Vec<SemanticDocument>,
    last_compaction_stats: &mut Option<CompactionStats>,
    compaction_attempts: &mut usize,
    compaction_events: &mut Vec<CompactionEventDiagnostics>,
) {
    let CompactionResult {
        history,
        stats,
        retrieval_docs,
    } = compaction;

    tracing::info!(
        "AI memory compaction applied: attempt={}, trigger={:?}, before_chars={}, after_chars={}, archived_docs={}, retrieved_docs={}",
        stats.attempt,
        stats.trigger,
        stats.before_chars,
        stats.after_chars,
        stats.archived_docs,
        stats.retrieved_docs
    );

    *chat_history = history;
    *semantic_docs = retrieval_docs;
    compaction_events.push(CompactionEventDiagnostics::from_stats(&stats));
    *last_compaction_stats = Some(stats);
    *compaction_attempts += 1;
}

type DynamicIndexBuilderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<DynamicContextIndex>, ExecuteAiStepError>> + 'a>>;

async fn run_prompt_loop_with_client<C, B>(
    client: &C,
    config: &NormalizedAiExecutionConfig,
    preamble: &str,
    task_message: &str,
    mut build_dynamic_index: B,
) -> Result<ExecuteAiStepResult, ExecuteAiStepError>
where
    C: rig::client::CompletionClient,
    C::CompletionModel: 'static,
    B: for<'a> FnMut(&'a C, &'a [SemanticDocument]) -> DynamicIndexBuilderFuture<'a>,
{
    let tool_server_handle = resolve_tool_server_handle(&config.tools).await?;
    let prompt_message = Message::user(task_message.to_string());
    let mut chat_history: Vec<Message> = Vec::new();
    let mut compaction_attempts = 0usize;
    let mut last_compaction_stats: Option<CompactionStats> = None;
    let mut semantic_docs: Vec<SemanticDocument> = Vec::new();
    let mut compaction_events: Vec<CompactionEventDiagnostics> = Vec::new();
    let budget_tracker = TokenBudgetTracker::default();

    loop {
        let dynamic_context_index = match build_dynamic_index(client, &semantic_docs).await {
            Ok(index) => index,
            Err(error) => {
                tracing::warn!(
                    "Skipping dynamic context index for this attempt due to index build failure: {}",
                    error
                );
                None
            }
        };

        let mut agent_builder =
            client
                .agent(config.model.clone())
                .preamble(preamble)
                .hook(TaskDoneAndMemoryHook {
                    budget_tracker: budget_tracker.clone(),
                });

        if let Some(index) = dynamic_context_index {
            agent_builder = agent_builder.dynamic_context(DYNAMIC_CONTEXT_SAMPLE_DOCS, index);
        }

        let agent = agent_builder
            .tool_server_handle(tool_server_handle.clone())
            .build();

        let response = agent
            .prompt(prompt_message.clone())
            .with_history(&mut chat_history)
            .with_tool_concurrency(1)
            .extended_details()
            .max_turns(config.max_steps)
            .await;

        match response {
            Ok(final_response) => {
                budget_tracker.update(final_response.total_usage, &prompt_message, &chat_history);
                return Ok(ExecuteAiStepResult {
                    data: Some(serde_json::Value::String(final_response.output.to_string())),
                    compaction_events,
                });
            }
            Err(PromptError::PromptCancelled {
                chat_history: cancelled_history,
                reason,
            }) => {
                if let Some(output) = task_done_output(&reason) {
                    return Ok(ExecuteAiStepResult {
                        data: Some(serde_json::Value::String(output)),
                        compaction_events,
                    });
                }

                let trigger = if is_proactive_cancel_reason(&reason) {
                    Some(MemoryTrigger::Proactive)
                } else if is_context_limit_error_text(&reason) {
                    Some(MemoryTrigger::ReactiveProviderError)
                } else {
                    None
                };

                if let Some(trigger) = trigger {
                    let compaction = compact_and_retry(
                        client,
                        config,
                        &prompt_message,
                        cancelled_history.as_ref(),
                        compaction_attempts,
                        last_compaction_stats.as_ref(),
                        trigger,
                    )
                    .await?;
                    apply_compaction_result(
                        compaction,
                        &mut chat_history,
                        &mut semantic_docs,
                        &mut last_compaction_stats,
                        &mut compaction_attempts,
                        &mut compaction_events,
                    );
                    continue;
                }

                return Err(ExecuteAiStepError::Execution(reason));
            }
            Err(error) => {
                let error_message = error.to_string();

                if is_context_limit_error_text(&error_message) {
                    let compaction = compact_and_retry(
                        client,
                        config,
                        &prompt_message,
                        &chat_history,
                        compaction_attempts,
                        last_compaction_stats.as_ref(),
                        MemoryTrigger::ReactiveProviderError,
                    )
                    .await?;
                    apply_compaction_result(
                        compaction,
                        &mut chat_history,
                        &mut semantic_docs,
                        &mut last_compaction_stats,
                        &mut compaction_attempts,
                        &mut compaction_events,
                    );
                    continue;
                }

                return Err(ExecuteAiStepError::Execution(error_message));
            }
        }
    }
}

fn as_execution_error(error: impl std::fmt::Display) -> ExecuteAiStepError {
    ExecuteAiStepError::Execution(error.to_string())
}

trait ApiKeyBaseUrlClient: rig::client::CompletionClient {
    fn build_from_config(
        config: &NormalizedAiExecutionConfig,
    ) -> std::result::Result<Self, rig::http_client::Error>
    where
        Self: Sized;
}

impl ApiKeyBaseUrlClient for rig::providers::openai::Client {
    fn build_from_config(
        config: &NormalizedAiExecutionConfig,
    ) -> std::result::Result<Self, rig::http_client::Error> {
        Self::builder()
            .api_key(config.api_key.clone())
            .base_url(config.endpoint.clone())
            .build()
    }
}

impl ApiKeyBaseUrlClient for rig::providers::anthropic::Client {
    fn build_from_config(
        config: &NormalizedAiExecutionConfig,
    ) -> std::result::Result<Self, rig::http_client::Error> {
        Self::builder()
            .api_key(config.api_key.clone())
            .base_url(config.endpoint.clone())
            .build()
    }
}

impl ApiKeyBaseUrlClient for rig::providers::gemini::Client {
    fn build_from_config(
        config: &NormalizedAiExecutionConfig,
    ) -> std::result::Result<Self, rig::http_client::Error> {
        Self::builder()
            .api_key(config.api_key.clone())
            .base_url(config.endpoint.clone())
            .build()
    }
}

fn build_no_dynamic_index<'a, C>(
    _client: &'a C,
    _docs: &'a [SemanticDocument],
) -> DynamicIndexBuilderFuture<'a>
where
    C: rig::client::CompletionClient,
{
    Box::pin(async { Ok(None) })
}

fn build_semantic_index<'a, C>(
    client: &'a C,
    embedding_model: &'static str,
    docs: &'a [SemanticDocument],
) -> DynamicIndexBuilderFuture<'a>
where
    C: EmbeddingsClient,
    C::EmbeddingModel: Clone + 'static,
{
    Box::pin(async move {
        build_dynamic_context_index(client, embedding_model, docs)
            .await
            .map_err(|e| {
                ExecuteAiStepError::Execution(format!(
                    "Failed to build semantic memory index: {}",
                    e
                ))
            })
    })
}

async fn build_client_and_run<C, E, F, B>(
    build_client: F,
    config: &NormalizedAiExecutionConfig,
    preamble: &str,
    task_message: &str,
    build_dynamic_index: B,
) -> Result<ExecuteAiStepResult, ExecuteAiStepError>
where
    C: rig::client::CompletionClient,
    C::CompletionModel: 'static,
    E: std::fmt::Display,
    F: FnOnce() -> Result<C, E>,
    B: for<'a> FnMut(&'a C, &'a [SemanticDocument]) -> DynamicIndexBuilderFuture<'a>,
{
    let client = build_client().map_err(as_execution_error)?;
    run_prompt_loop_with_client(&client, config, preamble, task_message, build_dynamic_index).await
}

async fn run_plain_base_url_client<C>(
    config: &NormalizedAiExecutionConfig,
    preamble: &str,
    task_message: &str,
) -> Result<ExecuteAiStepResult, ExecuteAiStepError>
where
    C: ApiKeyBaseUrlClient,
    C::CompletionModel: 'static,
{
    build_client_and_run(
        || C::build_from_config(config),
        config,
        preamble,
        task_message,
        build_no_dynamic_index,
    )
    .await
}

async fn run_semantic_base_url_client<C>(
    config: &NormalizedAiExecutionConfig,
    preamble: &str,
    task_message: &str,
    embedding_model: &'static str,
) -> Result<ExecuteAiStepResult, ExecuteAiStepError>
where
    C: ApiKeyBaseUrlClient + EmbeddingsClient,
    C::CompletionModel: 'static,
    C::EmbeddingModel: Clone + 'static,
{
    build_client_and_run(
        || C::build_from_config(config),
        config,
        preamble,
        task_message,
        move |client, docs| build_semantic_index(client, embedding_model, docs),
    )
    .await
}

async fn run_with_provider(
    config: &NormalizedAiExecutionConfig,
    preamble: &str,
    task_message: &str,
) -> Result<ExecuteAiStepResult, ExecuteAiStepError> {
    use rig::providers::{anthropic, azure, gemini, openai};

    match &config.llm_protocol {
        LlmProtocol::OpenAICompat => {
            run_semantic_base_url_client::<openai::Client>(
                config,
                preamble,
                task_message,
                openai::TEXT_EMBEDDING_3_SMALL,
            )
            .await
        }
        LlmProtocol::Anthropic => {
            run_plain_base_url_client::<anthropic::Client>(config, preamble, task_message).await
        }
        LlmProtocol::GoogleAi => {
            run_semantic_base_url_client::<gemini::Client>(
                config,
                preamble,
                task_message,
                gemini::EMBEDDING_004,
            )
            .await
        }
        LlmProtocol::AzureOpenAi => {
            build_client_and_run(
                || -> Result<azure::Client, rig::http_client::Error> {
                    azure::Client::builder()
                        .api_key(azure::AzureOpenAIAuth::ApiKey(config.api_key.clone()))
                        .azure_endpoint(config.endpoint.clone())
                        .build()
                },
                config,
                preamble,
                task_message,
                move |client, docs| {
                    build_semantic_index(client, azure::TEXT_EMBEDDING_3_SMALL, docs)
                },
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

    run_with_provider(&normalized, &preamble, &task_message).await
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
            tools: None,
            prompt: "Test".to_string(),
            working_dir: PathBuf::from("/tmp/project"),
            llm_protocol: "openai".to_string(),
        });

        assert_eq!(normalized.tools, get_default_cli_tools());
    }

    #[test]
    fn test_explicit_tools_are_forwarded() {
        let normalized = NormalizedAiExecutionConfig::from(ExecuteAiStepConfig {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "test-key".to_string(),
            model: "gpt-4o".to_string(),
            system_prompt: None,
            max_steps: None,
            tools: Some(vec!["bash".to_string(), "glob".to_string()]),
            prompt: "Test".to_string(),
            working_dir: PathBuf::from("/tmp/project"),
            llm_protocol: "openai".to_string(),
        });

        assert_eq!(normalized.tools, vec!["bash".to_string(), "glob".to_string()]);
    }

    #[test]
    fn test_task_done_reason_mapping() {
        let reason = format!("{TASK_DONE_REASON_PREFIX}Summary: done");
        assert_eq!(task_done_output(&reason), Some("Summary: done".to_string()));
    }

    #[test]
    fn test_non_task_done_reason_not_mapped() {
        assert_eq!(task_done_output("cancelled for another reason"), None);
    }

    #[test]
    fn test_compaction_event_diagnostics_from_stats() {
        let stats = CompactionStats {
            attempt: 3,
            trigger: MemoryTrigger::ReactiveProviderError,
            before_chars: 45_000,
            after_chars: 22_000,
            archived_docs: 7,
            retrieved_docs: 4,
        };

        let event = CompactionEventDiagnostics::from_stats(&stats);
        assert_eq!(event.attempt, 3);
        assert_eq!(event.trigger, "ReactiveProviderError");
        assert_eq!(event.before_chars, 45_000);
        assert_eq!(event.after_chars, 22_000);
        assert_eq!(event.archived_docs, 7);
        assert_eq!(event.retrieved_docs, 4);
    }

    #[test]
    fn test_build_memory_exhaustion_error_contains_structured_diagnostics() {
        let stats = CompactionStats {
            attempt: 5,
            trigger: MemoryTrigger::Proactive,
            before_chars: 88_000,
            after_chars: 54_000,
            archived_docs: 6,
            retrieved_docs: 7,
        };

        let error = build_memory_exhaustion_error(5, Some(&stats));
        let diagnostics = error
            .memory_exhaustion_diagnostics()
            .expect("expected structured memory exhaustion diagnostics");

        assert_eq!(diagnostics.attempts, 5);
        assert_eq!(diagnostics.trigger.as_deref(), Some("Proactive"));
        assert_eq!(diagnostics.before_chars, Some(88_000));
        assert_eq!(diagnostics.after_chars, Some(54_000));
        assert_eq!(diagnostics.archived_docs, Some(6));
        assert_eq!(diagnostics.retrieved_docs, Some(7));
        assert_eq!(diagnostics.soft_char_budget, SOFT_CONTEXT_CHAR_BUDGET);
        assert_eq!(diagnostics.soft_token_budget, SOFT_CONTEXT_TOKEN_BUDGET);
    }

    #[test]
    fn test_apply_compaction_result_records_event() {
        let compaction = CompactionResult {
            history: vec![Message::user("retained".to_string())],
            stats: CompactionStats {
                attempt: 2,
                trigger: MemoryTrigger::ReactiveProviderError,
                before_chars: 50_000,
                after_chars: 24_000,
                archived_docs: 5,
                retrieved_docs: 3,
            },
            retrieval_docs: vec![SemanticDocument {
                id: "doc-1".to_string(),
                text: "retrieved".to_string(),
            }],
        };

        let mut chat_history = Vec::new();
        let mut semantic_docs = Vec::new();
        let mut last_compaction_stats: Option<CompactionStats> = None;
        let mut compaction_attempts = 0usize;
        let mut compaction_events = Vec::new();

        apply_compaction_result(
            compaction,
            &mut chat_history,
            &mut semantic_docs,
            &mut last_compaction_stats,
            &mut compaction_attempts,
            &mut compaction_events,
        );

        assert_eq!(chat_history.len(), 1);
        assert_eq!(semantic_docs.len(), 1);
        assert_eq!(compaction_attempts, 1);
        assert_eq!(compaction_events.len(), 1);
        assert_eq!(compaction_events[0].attempt, 2);
        assert_eq!(compaction_events[0].trigger, "ReactiveProviderError");
        assert!(last_compaction_stats.is_some());
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

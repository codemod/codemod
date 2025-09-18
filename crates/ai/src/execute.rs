use std::path::PathBuf;

use coro_core::{
    agent::{AgentCore, AgentExecution},
    error::Error,
    AgentConfig, ModelParams, OutputMode, Protocol, ResolvedLlmConfig,
};

use crate::output::CliOutputHandler;

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

pub async fn execute_ai_step(config: ExecuteAiStepConfig) -> Result<AgentExecution, Error> {
    let agent_config = AgentConfig {
        system_prompt: config.system_prompt.clone(),
        max_steps: config.max_steps.unwrap_or(30),
        enable_lakeview: config.enable_lakeview.unwrap_or(false),
        tools: crate::tools::registry::get_default_cli_tools(),
        output_mode: OutputMode::Debug,
    };

    let llm_config = ResolvedLlmConfig::new(
        match config.llm_protocol.as_str() {
            "openai" => Protocol::OpenAICompat,
            "anthropic" => Protocol::Anthropic,
            "google_ai" => Protocol::GoogleAI,
            "azure_openai" => Protocol::AzureOpenAI,
            _ => Protocol::Custom(config.llm_protocol),
        },
        config.endpoint,
        config.api_key,
        config.model,
    )
    .with_params(ModelParams {
        max_tokens: Some(200000),
        temperature: Some(0.7),
        top_p: Some(1.0),
        top_k: None,
        stop_sequences: None,
    });

    let tool_registry = crate::tools::registry::create_cli_tool_registry();

    let mut agent = AgentCore::new_with_output_and_registry(
        agent_config,
        llm_config,
        Box::new(CliOutputHandler::default()),
        tool_registry,
        None,
    )
    .await?;

    let output = agent
        .execute_task_with_context(&config.prompt, &config.working_dir)
        .await?;

    Ok(output)
}

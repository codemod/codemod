//! `agent(prompt, options)`: hand a prompt to a coding agent.
//!
//! Order, as in the YAML engine: the preferred agent (`--agent` or
//! `LLM_AGENT`) if installed; hand-off to a parent coding agent when the
//! workflow runs inside one; an agent the person picks; then the built-in
//! agent when an API key is configured. Otherwise the prompt is printed as
//! instructions.

use std::io::{BufRead, BufReader};
use std::process::Stdio;

use butterflow_core::ai_handoff::{
    build_agent_command, detect_parent_coding_agent, discover_installed_agents,
    find_agent_executable, resolve_agent_name, DetectionConfidence,
};
use codemod_ai::execute::{execute_ai_step, ExecuteAiStepConfig};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{block_on, TaskContext, TaskError, TaskOutcome, TaskResult};
use crate::interaction::AgentChoice;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Request {
    prompt: String,
    system_prompt: Option<String>,
    model: Option<String>,
    max_steps: Option<usize>,
    tools: Option<Vec<String>>,
    endpoint: Option<String>,
    api_key: Option<String>,
    llm_protocol: Option<String>,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    let run = task.run;
    if run.dry_run {
        task.log("skipped: agents do not run in a dry run");
        return Ok(TaskOutcome::new(json!({ "skipped": true })));
    }

    let preferred = run
        .services
        .agent
        .clone()
        .or_else(|| std::env::var("LLM_AGENT").ok());
    if let Some(name) = preferred {
        match resolve_agent_name(&name) {
            Some(canonical) => match find_agent_executable(canonical) {
                Some(executable) => return launch(task, canonical, &executable, &request),
                None => task.log(format!(
                    "agent '{canonical}' is not installed; trying others"
                )),
            },
            None => task.log(format!("unknown agent '{name}'; trying others")),
        }
    }

    if detect_parent_coding_agent().confidence == DetectionConfidence::Detected {
        instructions(task, &request);
        return Ok(TaskOutcome::new(json!({ "handedOff": true })));
    }

    if run.interaction.is_interactive() {
        let installed = discover_installed_agents();
        let choices: Vec<AgentChoice> = installed
            .iter()
            .map(|agent| AgentChoice {
                name: agent.canonical.to_string(),
                label: agent.label.to_string(),
                available: agent.is_available(),
            })
            .collect();
        if let Some(choice) = run.interaction.choose_agent(&choices) {
            if let Some(executable) = resolve_agent_name(&choice).and_then(find_agent_executable) {
                let canonical = resolve_agent_name(&choice).unwrap_or_default();
                return launch(task, canonical, &executable, &request);
            }
        }
    }

    let Some(api_key) = request
        .api_key
        .clone()
        .or_else(|| std::env::var("LLM_API_KEY").ok())
    else {
        instructions(task, &request);
        task.log("no agent available and no LLM_API_KEY set; the prompt above was not run");
        return Ok(TaskOutcome::new(json!({ "handedOff": false })));
    };
    let protocol = request
        .llm_protocol
        .clone()
        .or_else(|| std::env::var("LLM_PROVIDER").ok())
        .unwrap_or_else(|| "openai".into());
    let endpoint = request
        .endpoint
        .clone()
        .or_else(|| std::env::var("LLM_BASE_URL").ok())
        .unwrap_or_else(|| default_endpoint(&protocol).into());
    let result = block_on(execute_ai_step(ExecuteAiStepConfig {
        endpoint,
        api_key,
        model: request
            .model
            .clone()
            .or_else(|| std::env::var("LLM_MODEL").ok())
            .unwrap_or_else(|| "gpt-4o".into()),
        system_prompt: request.system_prompt.clone(),
        max_steps: request.max_steps,
        tools: request.tools.clone(),
        prompt: request.prompt.clone(),
        working_dir: run.target.clone(),
        llm_protocol: protocol,
    }))
    .map_err(|e| TaskError::Failed(format!("the agent failed: {e}")))?;
    Ok(TaskOutcome::new(
        json!({ "data": result.data.unwrap_or(Value::Null) }),
    ))
}

fn default_endpoint(protocol: &str) -> &'static str {
    match protocol {
        "anthropic" => "https://api.anthropic.com",
        "google_ai" => "https://generativelanguage.googleapis.com/v1beta",
        "azure_openai" => "https://api.openai.com",
        _ => "https://api.openai.com/v1",
    }
}

fn instructions(task: &TaskContext<'_>, request: &Request) {
    task.log("[AI INSTRUCTIONS]");
    if let Some(system_prompt) = &request.system_prompt {
        task.log(system_prompt.clone());
    }
    task.log(request.prompt.clone());
    task.log("[/AI INSTRUCTIONS]");
}

fn launch(
    task: &TaskContext<'_>,
    canonical: &str,
    executable: &std::path::Path,
    request: &Request,
) -> TaskResult {
    let mut command = build_agent_command(
        canonical,
        executable,
        &request.prompt,
        request.system_prompt.as_deref(),
        &task.run.target,
    )
    .ok_or_else(|| TaskError::Failed(format!("cannot build a command for agent '{canonical}'")))?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if matches!(canonical, "claude-code" | "codex" | "opencode") {
        command.stdin(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .map_err(|e| TaskError::Failed(format!("cannot start agent '{canonical}': {e}")))?;
    drop(child.stdin.take());
    let stderr = child.stderr.take();
    let events = std::sync::Arc::clone(&task.run.events);
    let position = task.position;
    let stderr_reader = std::thread::spawn(move || {
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                events.emit(crate::events::Event::StepLog { position, line });
            }
        }
    });
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if !line.trim().is_empty() {
                task.log(line);
            }
        }
    }
    let _ = stderr_reader.join();
    let status = child
        .wait()
        .map_err(|e| TaskError::Failed(format!("agent '{canonical}' failed: {e}")))?;
    if !status.success() {
        return Err(TaskError::Failed(format!(
            "agent '{canonical}' exited with {status}"
        )));
    }
    Ok(TaskOutcome::new(json!({ "agent": canonical })))
}

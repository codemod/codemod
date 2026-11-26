//! `agent(prompt, options)`: hand a prompt to a coding agent.
//!
//! Which agent runs it:
//!
//! 1. the one the step names (`agent: "codex"`), or the run's (`--agent`,
//!    `LLM_AGENT`); it must be installed;
//! 2. when the workflow itself runs inside a coding agent, that agent: the
//!    prompt is handed back to it in the step's log, and the step succeeds;
//! 3. one the person picks, when the run is interactive;
//! 4. the built-in agent, when an API key is configured.
//!
//! Otherwise the prompt is logged as instructions and nothing runs.
//!
//! A read-only step does not run an agent that cannot be kept read-only:
//! naming one fails the step, and it is not offered to pick.
//!
//! Before an agent starts, the run asks ([`Approval::Agent`]): the prompt is
//! the package's, and an agent with full autonomy runs whatever commands it
//! is told to. An unattended run refuses without `--approve`, as it does a
//! `shell()` step.

use std::path::{Path, PathBuf};

use codemod_agents::{parent_agent, Adapter, Autonomy, ReadOnly, Run, RunError, Task};
use codemod_ai::execute::{execute_ai_step, ExecuteAiStepConfig};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{block_on, not_approved, TaskContext, TaskError, TaskOutcome, TaskResult};
use crate::events::Event;
use crate::interaction::{AgentChoice, Approval, Decision};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Request {
    prompt: String,
    /// The coding agent to run: an id or alias (`claude`, `codex`, ...).
    agent: Option<String>,
    system_prompt: Option<String>,
    model: Option<String>,
    /// `"full"` (default) or `"read-only"`.
    autonomy: Option<String>,
    /// Stop the agent after this many seconds.
    timeout_seconds: Option<u64>,
    // The built-in agent's options.
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
    let autonomy = match request.autonomy.as_deref() {
        None | Some("full") => Autonomy::Full,
        Some("read-only") => Autonomy::ReadOnly,
        Some(other) => {
            return Err(TaskError::Failed(format!(
                "unknown autonomy '{other}'; use \"full\" or \"read-only\""
            )))
        }
    };

    let named = request
        .agent
        .clone()
        .or_else(|| run.services.agent.clone())
        .or_else(|| std::env::var("LLM_AGENT").ok())
        .filter(|name| !name.trim().is_empty());
    if let Some(name) = named {
        let agent = codemod_agents::find(&name).ok_or_else(|| {
            let known: Vec<&str> = codemod_agents::agents().map(|a| a.id()).collect();
            TaskError::Failed(format!(
                "unknown coding agent '{name}'; supported: {}",
                known.join(", ")
            ))
        })?;
        return launch(task, agent, &request, autonomy);
    }

    if let Some(parent) = parent_agent() {
        instructions(task, &request);
        task.log(format!(
            "handed to {}, which runs this workflow",
            parent.adapter.label()
        ));
        return Ok(TaskOutcome::new(
            json!({ "handedOff": true, "agent": parent.adapter.id() }),
        ));
    }

    if run.interaction.is_interactive() {
        let choices: Vec<AgentChoice> = codemod_agents::agents()
            .map(|agent| AgentChoice {
                name: agent.id().to_string(),
                label: agent.label().to_string(),
                available: can_run(agent, autonomy) && agent.installed().is_some(),
            })
            .collect();
        let step = task.at().step;
        let ask = crate::interaction::Ask {
            id: format!("agent:{step}"),
            step: Some(step),
            stop: Some(std::sync::Arc::clone(&task.cancel)),
        };
        if let Some(choice) = run.interaction.pick_agent(&ask, &choices) {
            let agent = codemod_agents::find(&choice)
                .ok_or_else(|| TaskError::Failed(format!("unknown coding agent '{choice}'")))?;
            return launch(task, agent, &request, autonomy);
        }
    }

    built_in(task, &request)
}

/// Whether the agent may run a task with this autonomy: a read-only task
/// needs an agent that can be kept read-only.
fn can_run(agent: &dyn Adapter, autonomy: Autonomy) -> bool {
    autonomy == Autonomy::Full || agent.read_only() != ReadOnly::Unsupported
}

/// Ask before an agent gets the step's prompt. `endpoint` is where the
/// built-in agent sends it (and its API key).
fn approve(
    task: &TaskContext<'_>,
    label: &str,
    request: &Request,
    autonomy: Autonomy,
    endpoint: Option<&str>,
) -> Result<(), TaskError> {
    let approval = Approval::Agent {
        agent: label.to_string(),
        prompt: request.prompt.clone(),
        autonomy: match autonomy {
            Autonomy::Full => "full",
            Autonomy::ReadOnly => "read-only",
        }
        .to_string(),
        endpoint: endpoint.map(str::to_string),
    };
    match task.approve(&approval) {
        Decision::Approved => Ok(()),
        Decision::Denied => Err(TaskError::Failed(not_approved(
            task,
            &format!("running {label}"),
        ))),
        Decision::Deferred => Err(TaskError::AwaitingApproval(format!(
            "approve running {label}"
        ))),
    }
}

fn launch(
    task: &TaskContext<'_>,
    agent: &'static dyn Adapter,
    request: &Request,
    autonomy: Autonomy,
) -> TaskResult {
    let run = task.run;
    if !can_run(agent, autonomy) {
        let refused = RunError::ReadOnlyUnsupported {
            agent: agent.label(),
        };
        return Err(TaskError::Failed(refused.to_string()));
    }
    let executable = agent.installed().ok_or_else(|| {
        TaskError::Failed(format!(
            "{} is not installed (looked for {} on PATH)",
            agent.label(),
            agent.executables().join(", ")
        ))
    })?;
    approve(task, agent.label(), request, autonomy, None)?;
    let cwd = working_dir(&run.target);
    task.log(format!("running {}", agent.label()));
    let outcome = Run {
        adapter: agent,
        executable,
        task: Task {
            prompt: &request.prompt,
            system_prompt: request.system_prompt.as_deref(),
            model: request.model.as_deref(),
            cwd: &cwd,
            autonomy,
        },
        cancel: Some(std::sync::Arc::clone(&task.cancel)),
        timeout: request.timeout_seconds.map(std::time::Duration::from_secs),
    }
    .run(&mut |activity| {
        run.events.emit(Event::StepAgent {
            at: task.at(),
            agent: agent.id().to_string(),
            activity,
        })
    })
    .map_err(|error| {
        // The step's error is only a message: log the session the agent had
        // started and what it had used, which would be lost with it.
        if let Some(partial) = error.partial() {
            if let Some(session) = &partial.session_id {
                task.log(format!("{} session: {session}", agent.label()));
            }
            if !partial.usage.is_empty() {
                task.log(format!("usage until then: {}", json!(partial.usage)));
            }
        }
        match error {
            RunError::Canceled { .. } => TaskError::Canceled,
            other => TaskError::Failed(other.to_string()),
        }
    })?;
    let usage = (!outcome.usage.is_empty()).then(|| json!(outcome.usage));
    Ok(TaskOutcome::new(json!({
        "agent": outcome.agent,
        "result": outcome.result,
        "sessionId": outcome.session_id,
        "model": outcome.model,
        "usage": usage,
    })))
}

/// The directory an agent works in: the target, or a file target's directory.
fn working_dir(target: &Path) -> PathBuf {
    if target.is_file() {
        return target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
    }
    target.to_path_buf()
}

fn built_in(task: &TaskContext<'_>, request: &Request) -> TaskResult {
    // The machine's key goes where the machine says (LLM_BASE_URL,
    // LLM_PROVIDER), never where the workflow says.
    if request.api_key.is_none() && (request.endpoint.is_some() || request.llm_protocol.is_some()) {
        return Err(TaskError::Failed(
            "the step sets the built-in agent's endpoint or llmProtocol without an apiKey: LLM_API_KEY is only sent to LLM_BASE_URL (or the provider's default). Set LLM_BASE_URL and LLM_PROVIDER for the run, or give the step its own apiKey".into(),
        ));
    }
    let Some(api_key) = request
        .api_key
        .clone()
        .or_else(|| std::env::var("LLM_API_KEY").ok())
    else {
        instructions(task, request);
        task.log("no coding agent chosen and no LLM_API_KEY set; the prompt above was not run");
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
    // The built-in agent has a shell tool and does what the model at
    // `endpoint` says, and both the endpoint and the key may be the step's.
    approve(
        task,
        "the built-in agent",
        request,
        Autonomy::Full,
        Some(&endpoint),
    )?;
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
        working_dir: working_dir(&task.run.target),
        llm_protocol: protocol,
    }))
    .map_err(|e| TaskError::Failed(format!("the built-in agent failed: {e}")))?;
    Ok(TaskOutcome::new(json!({
        "agent": "built-in",
        "data": result.data.unwrap_or(Value::Null),
    })))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_step_runs_only_agents_that_can_be_kept_read_only() {
        let agent = |name: &str| codemod_agents::find(name).unwrap();
        assert!(can_run(agent("claude-code"), Autonomy::ReadOnly));
        assert!(can_run(agent("gemini"), Autonomy::ReadOnly));
        assert!(!can_run(agent("kimi"), Autonomy::ReadOnly));
        assert!(can_run(agent("kimi"), Autonomy::Full));
    }
}

//! Runs real, installed agents. Opt in with the agents to try, each with an
//! optional model:
//!
//! ```sh
//! CODEMOD_AGENTS_LIVE="claude-code=haiku,codex,opencode=opencode/big-pickle" \
//!   cargo test -p codemod-agents --test live -- --nocapture
//! ```
//!
//! Each agent answers a question in an empty directory, read-only when it
//! can be kept read-only.

use std::time::Duration;

use codemod_agents::{find, AgentEvent, Autonomy, ReadOnly, Run, Task};

#[test]
fn installed_agents_answer() {
    let Ok(list) = std::env::var("CODEMOD_AGENTS_LIVE") else {
        eprintln!("skipped: set CODEMOD_AGENTS_LIVE to run real agents");
        return;
    };
    let mut failures = Vec::new();
    for entry in list.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let (name, model) = match entry.split_once('=') {
            Some((name, model)) => (name, Some(model)),
            None => (entry, None),
        };
        let Some(agent) = find(name) else {
            panic!("unknown agent {name}");
        };
        let Some(executable) = agent.installed() else {
            failures.push(format!("{name}: not installed"));
            continue;
        };
        let cwd = tempfile::tempdir().unwrap();
        let mut events = Vec::new();
        let result = Run {
            adapter: agent,
            executable,
            task: Task {
                prompt: "What is 6 times 7? Reply with the number only.",
                system_prompt: Some("Answer as briefly as possible."),
                model,
                cwd: cwd.path(),
                autonomy: match agent.read_only() {
                    ReadOnly::Unsupported => Autonomy::Full,
                    _ => Autonomy::ReadOnly,
                },
            },
            cancel: None,
            timeout: Some(Duration::from_secs(300)),
        }
        .run(&mut |event| events.push(event));
        match result {
            Ok(outcome) => {
                eprintln!(
                    "{name}: {:?} (session {:?}, model {:?}, usage {:?}, {} events, {:.1}s)",
                    outcome.result,
                    outcome.session_id,
                    outcome.model,
                    outcome.usage,
                    events.len(),
                    outcome.duration.as_secs_f64()
                );
                if !outcome.result.as_deref().unwrap_or_default().contains("42") {
                    failures.push(format!("{name}: answered {:?}", outcome.result));
                }
                let raw: Vec<_> = events
                    .iter()
                    .filter(|e| {
                        matches!(
                            e,
                            AgentEvent::Output {
                                stream: codemod_agents::Stream::Stdout,
                                ..
                            }
                        )
                    })
                    .collect();
                if !raw.is_empty() {
                    failures.push(format!("{name}: unparsed output {raw:?}"));
                }
            }
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

use std::sync::Arc;

use butterflow_core::ai_handoff::AgentOption;
use butterflow_core::config::AgentSelectionCallback;
use inquire::Select;

pub fn create_agent_selection_callback() -> AgentSelectionCallback {
    Arc::new(Box::new(|agents: &[AgentOption]| {
        if agents.is_empty() {
            return None;
        }

        let mut options: Vec<AgentSelectItem> = agents
            .iter()
            .map(|a| AgentSelectItem {
                canonical: a.canonical,
                label: a.label,
                kind: if a.is_available() {
                    AgentSelectKind::Available
                } else {
                    AgentSelectKind::NotInstalled
                },
            })
            .collect();

        // Sort: available agents first, then "print prompt", then unavailable
        options.sort_by_key(|o| match o.kind {
            AgentSelectKind::Available => 0,
            AgentSelectKind::PrintPrompt => 1,
            AgentSelectKind::NotInstalled => 2,
        });

        // Insert "Print prompt" option between available and not-installed
        let insert_pos = options
            .iter()
            .position(|o| o.kind == AgentSelectKind::NotInstalled)
            .unwrap_or(options.len());
        options.insert(
            insert_pos,
            AgentSelectItem {
                canonical: "__print_prompt__",
                label: "Print prompt",
                kind: AgentSelectKind::PrintPrompt,
            },
        );

        let result = Select::new("Select a coding agent to execute the AI step:", options)
            .with_help_message("↑↓ to move, enter to select, esc to use built-in AI")
            .prompt_skippable();

        match result {
            Ok(Some(selected)) => match selected.kind {
                AgentSelectKind::Available => Some(selected.canonical.to_string()),
                AgentSelectKind::PrintPrompt => Some("__print_prompt__".to_string()),
                AgentSelectKind::NotInstalled => None,
            },
            _ => None,
        }
    }))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AgentSelectKind {
    Available,
    PrintPrompt,
    NotInstalled,
}

struct AgentSelectItem {
    canonical: &'static str,
    label: &'static str,
    kind: AgentSelectKind,
}

impl std::fmt::Display for AgentSelectItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            AgentSelectKind::Available => write!(f, "{}", self.label),
            AgentSelectKind::PrintPrompt => write!(f, "Print prompt"),
            AgentSelectKind::NotInstalled => {
                write!(f, "\x1b[2m{} (not installed)\x1b[0m", self.label)
            }
        }
    }
}

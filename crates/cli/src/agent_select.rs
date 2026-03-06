use std::sync::Arc;

use butterflow_core::ai_handoff::AgentOption;
use butterflow_core::config::AgentSelectionCallback;
use inquire::Select;

pub fn create_agent_selection_callback() -> AgentSelectionCallback {
    Arc::new(Box::new(|agents: &[AgentOption]| {
        if agents.is_empty() {
            return None;
        }

        let options: Vec<AgentSelectItem> = agents
            .iter()
            .map(|a| AgentSelectItem {
                canonical: a.canonical,
                label: a.label,
                available: a.is_available(),
            })
            .collect();

        // Only show the prompt if at least one agent is available
        let any_available = options.iter().any(|o| o.available);
        if !any_available {
            return None;
        }

        let result = Select::new("Select a coding agent to execute the AI step:", options)
            .with_help_message("↑↓ to move, enter to select, esc to use built-in AI")
            .prompt_skippable();

        match result {
            Ok(Some(selected)) if selected.available => Some(selected.canonical.to_string()),
            _ => None,
        }
    }))
}

struct AgentSelectItem {
    canonical: &'static str,
    label: &'static str,
    available: bool,
}

impl std::fmt::Display for AgentSelectItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.available {
            write!(f, "{}", self.label)
        } else {
            write!(f, "{} (not installed)", self.label)
        }
    }
}

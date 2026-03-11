use std::sync::Arc;

use butterflow_core::ai_handoff::AgentOption;
use butterflow_core::config::AgentSelectionCallback;
use console::style;
use inquire::Select;

pub fn create_agent_selection_callback() -> AgentSelectionCallback {
    Arc::new(Box::new(|agents: &[AgentOption]| {
        if agents.is_empty() {
            return None;
        }

        // Print a clear warning before the selection prompt
        eprintln!();
        eprintln!(
            "  {}",
            style("⚠  WARNING: Agents run with full write permissions")
                .yellow()
                .bold()
        );
        eprintln!(
            "  {}",
            style("Coding agents will be launched in auto-accept mode, allowing them").yellow()
        );
        eprintln!(
            "  {}",
            style("to edit files, run commands, and make changes without confirmation.").yellow()
        );
        eprintln!(
            "  {}",
            style("If you're uncomfortable with this, select \"Preview prompt\" to see the")
                .yellow()
        );
        eprintln!(
            "  {}",
            style("prompt first and run it in your agent manually.").yellow()
        );
        eprintln!();

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

        // Sort: available agents first, then special options, then unavailable
        options.sort_by_key(|o| match o.kind {
            AgentSelectKind::Available => 0,
            AgentSelectKind::PreviewPrompt => 1,
            AgentSelectKind::PrintPrompt => 2,
            AgentSelectKind::NotInstalled => 3,
        });

        // Insert special options between available and not-installed
        let insert_pos = options
            .iter()
            .position(|o| o.kind == AgentSelectKind::NotInstalled)
            .unwrap_or(options.len());
        options.insert(
            insert_pos,
            AgentSelectItem {
                canonical: "__preview_prompt__",
                label: "Preview prompt",
                kind: AgentSelectKind::PreviewPrompt,
            },
        );
        options.insert(
            insert_pos + 1,
            AgentSelectItem {
                canonical: "__print_prompt__",
                label: "Print prompt and skip",
                kind: AgentSelectKind::PrintPrompt,
            },
        );

        let result = Select::new("Select a coding agent to execute the AI step:", options)
            .with_help_message("↑↓ to move, enter to select, esc to use built-in AI")
            .prompt_skippable();

        match result {
            Ok(Some(selected)) => match selected.kind {
                AgentSelectKind::Available => Some(selected.canonical.to_string()),
                AgentSelectKind::PreviewPrompt => Some("__preview_prompt__".to_string()),
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
    PreviewPrompt,
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
            AgentSelectKind::PreviewPrompt => write!(f, "{}", style("Preview prompt").cyan()),
            AgentSelectKind::PrintPrompt => write!(f, "Print prompt and skip"),
            AgentSelectKind::NotInstalled => {
                write!(
                    f,
                    "{}",
                    style(format!("{} (not installed)", self.label)).dim()
                )
            }
        }
    }
}

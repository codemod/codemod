//! One adapter per coding agent.

mod claude;
mod claude_like;
mod codex;
mod copilot;
mod droid;
mod google;
mod goose;
mod kimi;
mod openclaw;
mod opencode;
mod text;

use crate::adapter::Adapter;

/// Every supported agent, in the order they are offered.
///
/// The ids are also listed in the `CodingAgent` type of
/// `crates/cli/npm/types/modules/workflow.d.ts` and in the agents table of
/// `docs/community/workflows/reference.mdx`; `tests/agents.rs` fails when
/// they differ.
pub(crate) static ADAPTERS: &[&dyn Adapter] = &[
    &claude::ClaudeCode,
    &codex::Codex,
    &claude_like::Cursor,
    &google::GeminiCli,
    &google::Antigravity,
    &opencode::OpenCode,
    &copilot::Copilot,
    &claude_like::Amp,
    &claude_like::QwenCode,
    &goose::Goose,
    &droid::Droid,
    &kimi::Kimi,
    &text::CRUSH,
    &openclaw::OpenClaw,
    &text::AIDER,
    &text::KIRO,
    &text::AUGGIE,
    &text::CLINE,
];

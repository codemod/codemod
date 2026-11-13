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

pub use claude::ClaudeCode;
pub use claude_like::{Amp, Cursor, QwenCode};
pub use codex::Codex;
pub use copilot::Copilot;
pub use droid::Droid;
pub use google::{Antigravity, GeminiCli};
pub use goose::Goose;
pub use kimi::Kimi;
pub use openclaw::OpenClaw;
pub use opencode::OpenCode;
pub use text::{Aider, Auggie, Cline, Crush, Kiro};

use crate::adapter::Adapter;

/// Every supported agent, in the order they are offered.
pub static ADAPTERS: &[&dyn Adapter] = &[
    &ClaudeCode,
    &Codex,
    &Cursor,
    &GeminiCli,
    &Antigravity,
    &OpenCode,
    &Copilot,
    &Amp,
    &QwenCode,
    &Goose,
    &Droid,
    &Kimi,
    &Crush,
    &OpenClaw,
    &Aider,
    &Kiro,
    &Auggie,
    &Cline,
];

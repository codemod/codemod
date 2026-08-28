---
name: codemod-cli-tui
description: Use when changing Codemod CLI commands, terminal UI, workflow command behavior, auth/login, publish/search/unpublish, init templates, report server, npm CLI wrapper, quiet-mode output, terminal dependencies, or command tests.
---

# Codemod CLI And TUI

## Key Areas

- Commands: `crates/cli/src/commands`
- TUI: `crates/cli/src/tui`
- Templates: `crates/cli/src/templates`
- CLI helpers: `crates/cli/src/utils`
- npm wrapper: `crates/cli/npm`
- Report UI: `crates/cli/report-ui`

## Rules

- The CLI owns all user-facing terminal output for the repo. Other crates and packages must expose
  structured data, errors, events, reports, or logs for CLI routing.
- Preserve non-interactive behavior; CI-facing commands must not prompt unexpectedly.
- Keep command-specific logic in command modules and shared behavior in `utils` only when reused.
- TUI/quiet mode must not write workflow logs, prompts, agent output, spinners, or progress directly
  to stdout/stderr while `WorkflowOutputSettings.quiet` is true.
- Direct writes outside CLI-owned output paths can leak to stdout while the TUI is shown or bypass
  JSONL formatting.
- Route quiet-mode interaction through workflow/TUI events and task logs.
- Template changes should update generated package files and user-facing template docs together.
- npm wrapper changes need wrapper tests because users install through the package, not only the Rust
  binary.

## Validation

- Rust CLI: `cargo test -p codemod`
- npm wrapper: `pnpm --filter codemod test`
- Terminal dependency graph: `bash ./scripts/check-single-crossterm.sh`
- Report UI build: `pnpm --filter codemod-report-ui build`

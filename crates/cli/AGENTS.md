# CLI And TUI Rules

This crate owns the `codemod` binary, command handlers, package templates, auth, publish/search
commands, workflow subcommands, report UI serving, and terminal UI.

## CLI Behavior

- Keep command parsing and user-facing behavior in `src/commands/*`; keep reusable helpers in
  `src/utils/*` only when more than one command actually needs them.
- Preserve non-interactive behavior. Commands used in CI must not block on prompts.
- When changing package templates in `src/templates`, test the generated shape or update the related
  template README/workflow files together.
- npm wrapper behavior lives under `crates/cli/npm`; keep wrapper tests in `crates/cli/npm/tests`.

## TUI And Output

- The CLI owns all user-facing terminal output for the repo. Other crates and packages must expose
  structured data, errors, events, reports, or logs; the CLI routes them to text, TUI, JSONL, or task
  logs.
- TUI/quiet mode owns the terminal. Do not write workflow logs, agent output, prompts, spinners, or
  progress directly to stdout/stderr while `WorkflowOutputSettings.quiet` is true.
- Direct writes outside CLI-owned output paths can leak to stdout while the TUI is shown or bypass
  JSONL formatting.
- Route interaction through workflow/TUI events and task logs. Non-quiet text runs may print only
  through CLI-owned output paths.
- For terminal dependencies, keep `crossterm` usage centralized; CI checks this with
  `bash ./scripts/check-single-crossterm.sh`.

## Validation

- CLI crate: `cargo test -p codemod`
- CLI wrapper: `pnpm --filter codemod test`
- Terminal dependency check: `bash ./scripts/check-single-crossterm.sh`
- TUI perf-sensitive changes: inspect or run the scripts under `scripts/` and `crates/cli/src/tui`.

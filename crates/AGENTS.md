# Rust Workspace Rules

This subtree contains the Rust workspace. Keep crate boundaries explicit and prefer existing
workspace dependencies from the root `Cargo.toml` over adding direct versions inside crates.

## Change Discipline

- Use existing crate roles: `models` for data contracts, `core` for workflow execution, `state` for
  persistence adapters, `runners` for runtime execution, `scheduler` for scheduling logic, `cli` for
  command/UI surfaces, and `testing-utils` for shared test helpers.
- Avoid adding cross-crate dependencies that invert those roles. If a lower-level crate needs a
  higher-level type, move the shared contract down instead.
- Keep public model changes backward-compatible unless the task is explicitly a breaking change.
- For async code, preserve cancellation and error propagation. Do not hide task failures behind logs.
- Do not write directly to stdout/stderr from non-CLI crates. Return structured data, errors, events,
  reports, or log records and let `crates/cli` decide whether to render text, TUI updates, JSONL, or
  task logs.
- For filesystem or git operations, prefer existing helpers in `crates/core/src/*_ops.rs`,
  `crates/cli/src/utils`, or `testing-utils`.

## Validation

- Targeted crate: `cargo test -p <crate-name>`
- Workspace: `cargo test`
- Format: `cargo fmt --check`
- Lint: `cargo clippy --tests --no-deps -- -D warnings`
- If workflow models or schemas changed: `cargo xtask schema`, then inspect `git status --short`.

# Workflow Engine Rules

This crate owns workflow execution, step execution, git/file operations, reporting, structured logs,
JSSG execution, nested codemods, and runtime services.

## Contracts

- Keep workflow data contracts in `crates/models`; do not define parallel model structs in `core`
  unless they are strictly internal runtime state.
- State writes must go through the shared state abstractions. Preserve JSON value semantics and schema
  validation behavior.
- Be careful with matrix recompilation, manual triggers, task identity, and dependency resolution;
  these are user-visible workflow semantics.
- Preserve structured logs and reports when changing execution paths. Logs are part of debugging and
  hosted-platform handoff behavior.
- Never print directly to stdout/stderr from core. Emit structured logs, reports, task events, or
  errors and let `crates/cli` route them. Printing here can leak through the TUI or bypass JSONL
  formatting.
- Use deterministic temp directories and fixtures in tests. Avoid relying on the developer's git
  state except in tests that explicitly construct a repository.

## Validation

- Core tests: `cargo test -p butterflow-core`
- Cross-crate workflow changes: `cargo test -p butterflow-core -p butterflow-models -p butterflow-state`
- Schema-impacting model changes: run `cargo xtask schema` from the repository root.

---
name: codemod-rust-workspace
description: Use when working on Codemod's Rust workspace, including workflow engine crates, models, state, runners, CLI-adjacent Rust crates, schema generation, Cargo features, Rust tests, clippy, rustfmt, or CI parity.
---

# Codemod Rust Workspace

## Start Here

1. Read the nearest `AGENTS.md` for the crate you are changing.
2. Identify the crate role before editing:
   - `codemod-workflow`: the workflow engine: `workflow.ts` build, runtime, tasks, run
     orchestration, and the workflow.yaml compatibility translator.
   - `butterflow-models`: workflow.yaml schema/data contracts.
   - `butterflow-core`: shared services: registry, JSSG runner, shards, git, diffs, reports.
   - `butterflow-state`: run storage (`RunStore`) and its adapters.
   - `butterflow-runners`: the direct shell runner.
   - `codemod`: CLI command surface.
   - `codemod-sandbox`: JSSG runtime and sandbox.
   - `testing-utils`: shared test helpers.
3. Prefer workspace dependencies in root `Cargo.toml`.

## Implementation Rules

- Keep model contracts in `crates/models`; avoid duplicate public structs in higher-level crates.
- Preserve async cancellation and error propagation.
- Avoid logging-only failures for user-visible workflow behavior.
- Do not write directly to stdout/stderr from non-CLI crates. Return structured data, errors, events,
  reports, or log records and let `crates/cli` choose text, TUI, JSONL, or task-log routing.
- Keep feature gates meaningful, especially `docker`, `podman`, `wasm`, `native`, and `real-fs`.
- For schema-affecting model changes, run `cargo xtask schema` and review generated diffs.

## Validation

- Focused crate: `cargo test -p <crate>`
- Workspace: `cargo test`
- Format: `cargo fmt --check`
- Lint: `cargo clippy --tests --no-deps -- -D warnings`
- CI parity for terminal dependencies: `bash ./scripts/check-single-crossterm.sh`

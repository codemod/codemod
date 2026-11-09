# Agent Rules

This repository is the Codemod monorepo: a Rust CLI and workflow engine, a JavaScript/TypeScript
JSSG sandbox, shared TypeScript packages, semantic-analysis providers, and Mintlify docs.

## Repo Map

- `crates/cli`: Rust `codemod` binary, command handlers, templates, auth, publishing, workflow
  commands, report server, and TUI.
- `crates/core`, `crates/models`, `crates/state`, `crates/runners`, `crates/scheduler`: workflow
  engine, schemas, scheduling, state, runtime execution, and runner abstractions.
- `crates/codemod-sandbox`: Rust and TypeScript JavaScript sandbox used by JSSG, ast-grep
  integration, WASM/native packaging, and runtime module shims.
- `crates/language-*`, `crates/semantic-factory`, `crates/tree-sitter-loader`: semantic providers,
  provider factory, and parser loading.
- `packages/jssg-types`, `packages/jssg-utils`, `packages/tsconfig`: public TypeScript types,
  utilities, and shared TS config for codemod authors.
- `docs`: Mintlify docs.
- `scripts`: repository maintenance checks used by CI.

## Area Rules

- Read the nearest nested `AGENTS.md` before changing files in an area. Nested rules are intentionally
  narrow and override this file for their subtree.
- Keep changes scoped to the touched domain. Do not mix CLI, engine, docs, package, and semantic
  provider refactors unless the request requires it.
- Do not edit generated outputs unless the request is specifically about generated artifacts:
  `dist/`, `target/`, `crates/codemod-sandbox/js/factory.js`,
  `crates/codemod-sandbox/sandbox.wasm`, `crates/scheduler/npm/pkg`, tree-sitter WASM files, or
  package manager generated folders.
- When a Rust change affects exported workflow models or schemas, run `cargo xtask schema` and check
  for resulting committed schema changes.
- TypeScript and JavaScript formatting/linting are handled by `oxfmt` and `oxlint` from the repo root.
  Rust formatting/linting uses `cargo fmt` and `cargo clippy --tests --no-deps -- -D warnings`.
- Prefer targeted validation first, then broader validation when the change crosses crate or package
  boundaries.

## Docs gate (end of every user-facing change)

Before claiming feature or bugfix work done, classify impact and update docs. Load
`.agents/skills/codemod-docs/SKILL.md` when editing Mintlify or deciding the docs corpus.
See also `docs/AGENTS.md`.

| If the change affects… | Update in this repo | Also required |
|------------------------|---------------------|---------------|
| Human-facing CLI, JSSG, workflows, registry, or enterprise product behavior/concepts | Matching pages under `docs/` (+ `docs/docs.json` if nav changes) | — |
| Wish / in-app agent SDK, chat tools, or agent operating policy | — (skill-docs live in `codemod-app`) | **Heads-up the user** to update `codemod-app` → `packages/modern-ai/src/global-chat/skill-docs/` |
| Both | Mintlify here | Heads-up for Wish skill-docs in `codemod-app` |
| Neither (internal refactors, tests-only, no user/Wish surface) | Skip docs | Say so briefly |

### Cross-repo heads-up (required when Wish skill-docs may need changes)

Agents in this workspace usually cannot edit `codemod-app`. When Wish skill-docs may need an
update, **do not silently skip**. End the response with an explicit heads-up, for example:

> **Docs follow-up (`codemod-app` repo):** This change affects Wish / in-app agent contracts.
> Please update skill-docs under `packages/modern-ai/src/global-chat/skill-docs/` (and
> `skillRegistry` if needed). Open the `codemod-app` workspace (or ask me there) to apply the
> `wish-skill-docs` skill.

Never put Wish SDK method dumps, `load_skill` / `execute` contracts, or agent consent policy into
Mintlify beyond a short human Wish overview.

## Standard Validation

- Rust formatting: `cargo fmt --check`
- Rust lint: `cargo clippy --tests --no-deps -- -D warnings`
- Rust tests: `cargo test` or `cargo test -p <crate>`
- Terminal dependency check: `bash ./scripts/check-single-crossterm.sh`
- TypeScript/JavaScript lint: `pnpm lint`
- TypeScript/JavaScript format: `pnpm format`
- JSSG utils tests: `pnpm --filter @jssg/utils test`
- Scheduler npm tests: `pnpm --filter @codemod.com/butterflow-scheduler test`

## Output Discipline

- Only the CLI package may write user-facing output to the terminal. All other crates and packages
  must return structured data, errors, events, reports, or logs for `crates/cli` to route.
- Do not call `println!`, `eprintln!`, `console.log`, `console.error`, or write directly to
  stdout/stderr from engine, scheduler, sandbox, semantic-provider, package, or utility code.
- Direct terminal writes outside the CLI can leak to stdout while the TUI is shown and can bypass the
  JSONL formatter. Treat that as a correctness bug, not just noisy output.
- TUI/quiet mode owns the terminal. While `WorkflowOutputSettings.quiet` is true, route workflow
  logs, agent output, prompts, spinners, and progress through workflow/TUI events and task logs.
- Non-quiet text runs may print only through CLI-owned output paths.

## Repo-Local Skills

Use these skills when the task matches the area:

- `.agents/skills/codemod-rust-workspace/SKILL.md`: Rust workspace, workflow engine, models,
  scheduler, state, runners, schema, and CI parity.
- `.agents/skills/codemod-cli-tui/SKILL.md`: CLI commands, TUI, terminal output, templates, auth,
  package publishing, and npm wrapper behavior.
- `.agents/skills/codemod-jssg-sandbox/SKILL.md`: JSSG sandbox, ast-grep bindings, QuickJS/WASM,
  runtime modules, JSSG TypeScript packages, and codemod author APIs.
- `.agents/skills/codemod-semantic-providers/SKILL.md`: semantic analysis providers, language
  factory, tree-sitter loader, goto-definition/reference behavior, and provider tests.
- `.agents/skills/codemod-docs/SKILL.md`: Mintlify documentation, docs navigation, end-of-dev docs
  gate, and cross-repo heads-up when Wish skill-docs in `codemod-app` also need updates. Human
  product docs only for in-repo edits (see `docs/AGENTS.md`).

# Codemod untangle-codebase report

Analysis-only pass of the [untangle-codebase](https://github.com/caldoai/skills/blob/main/skills/untangle-codebase/SKILL.md) skill against `codemod/codemod`. No production code was changed.

| | |
|---|---|
| Generated | 2026-11-08 · America/Los_Angeles |
| Base | `main @ 0d66d565` |
| Mode | report only · Comments/Tests options deferred |
| Interactive twin | [`untangle-codebase.html`](./untangle-codebase.html) (best on desktop; GitHub mobile shows HTML as source) |

**36** problems · **10** proposed fixes · **0** code changes applied

## How to read this

The skill asks: *should these pieces change together?*

- **Hidden link** — must change together, but the code does not show it
- **Needless link** — tied together but change for different reasons
- **Surplus** — no reason of their own to change
- **Special case** — exists for one situation instead of a design that accounts for it

**Safety** = risk of applying the proposed fix. **Impact** = how much it helps agents. Prefer high impact / low–medium safety first.

## Priority matrix

| | Safety low | Safety medium | Safety high |
|---|---|---|---|
| **Impact high** | **Start here:** F2, F5 | **Next wave:** F1, F3, F4, F7 | **Plan carefully:** F6 |
| **Impact med** | F9, F10 | F8 | — |
| **Impact low** | included in F10 | — | — |

## Proposed fixes

### F1 · Make terminal ownership a single typed mode
**Rank 1** · CLI / Core / Runners · Impact **high** · Safety **medium**

Introduce one `RunPresentationMode { Text | Quiet | Tui | Jsonl }` applied once when constructing the engine/runner. Non-CLI crates return structured lines/events; CLI alone renders.

Solves: non-CLI stdout, quiet call-order convention, dual `WorkflowRunConfig` clones, JSONL/`println!` mix, `FileDiff::print` / `DirectRunner::with_quiet`, session attach mutating quiet.

### F2 · One generated capability & language catalog
**Rank 2** · Sandbox / Semantic / Types · Impact **high** · Safety **low**

Own module IDs, LLRT allowlists, publish externals, TS declarations, and extension→language maps in one catalog that generates Rust + TypeScript. Single `install_codemod_modules` helper for every engine.

### F3 · Typed matrix identity, cancel, and execution profile
**Rank 3** · Models / Core / Scheduler · Impact **high** · Safety **medium**

Replace string conventions with types (`MatrixMeta`, `TaskStatus::Canceled`, `ExecutionProfile`). Share one deterministic hashing **implementation**, but keep **separate inputs and metadata policies**: state-change fingerprints the workflow state that should trigger recompile; task identity fingerprints matrix items and continues to exclude `_meta_` fields (`test_meta_key_exclusion`) so shard identity does not churn when file metadata changes. Do not merge the two fingerprint semantics into one key.

### F4 · Pass an EventBus; one task-log writer
**Rank 4** · Core runtime · Impact **high** · Safety **medium**

Delete the process-global workflow event sink map. Inject an `EventBus`. Make `TaskStateService` the only path that mutates task logs and publishes log events.

### F5 · Shared post-run presenter for run / workflow / jssg
**Rank 5** · CLI · Impact **high** · Safety **low**

Keep package-resolve and raw-jssg as front doors only. Share one post-run presenter for metrics, report decision, and report server launch.

### F6 · Split Engine into typed runtime ports
**Rank 6** · Core · Impact **high** · Safety **high**

Carve `engine.rs` (~5248 LOC) into scheduler loop, step runtime, and task lifecycle, with one task executor for main loop and resume/trigger. Do after F1/F3/F4.

### F7 · One registry client and auth DTO
**Rank 7** · CLI / Core registry · Impact **high** · Safety **medium**

Collapse parallel token types; move publish HTTP into shared `RegistryClient`.

### F8 · Split harness_adapter by concern
**Rank 8** · CLI AI / harness · Impact **medium** · Safety **medium**

Break the ~6852-line harness god module into per-harness adapters plus shared lock/state. Also thin out `report_server` concerns.

### F9 · Keep models pure; move expression evaluation
**Rank 9** · Models / Core · Impact **medium** · Safety **low**

Leave AST/types in `butterflow-models`. Move evaluation and `CODEMOD_TASK_*` env binding into workflow-expr/core.

### F10 · Delete surplus and make generation honest
**Rank 10** · Sandbox / Semantic / Packages · Impact **medium** · Safety **low**

Remove dead speculative APIs / unused noop provider; fix generated-artifact honesty; force semantic fixtures through the factory; data-drive the js-astgrep language matrix in `init`.

## Recommended next steps

1. Land **F1 + F5** together (CLI presentation ownership).
2. Land **F2** with a CI inventory check.
3. Land **F3 + F4** before the Engine split.
4. Only then attempt **F6**.
5. Schedule **F7–F10** opportunistically.

Skill additions not run: **Comments**, **Tests**.

## All problems

Impact/safety below are the **parent fix** ratings (same source as the cards/matrix).

| ID | Category | Problem | Fix | Fix impact | Fix safety |
|---|---|---|---|---|---|
| non-cli-stdout | hidden-link | Non-CLI crates write stdout/stderr | F1 | high | medium |
| quiet-call-order | hidden-link | Quiet/TUI ownership is multi-site call order | F1 | high | medium |
| dual-run-config | hidden-link | Dual WorkflowRunConfig clones with partial dual-writes | F1 | high | medium |
| jsonl-println | special-case | `--format jsonl` still mixed with println summaries | F1 | high | medium |
| filediff-print | needless-link | FileDiff::print is presentation in core | F1 | high | medium |
| direct-runner-quiet | needless-link | DirectRunner prints based on quiet bit | F1 | high | medium |
| session-attach-quiet | hidden-link | Session attach mutates quiet + callbacks | F1 | high | medium |
| cap-sync | hidden-link | Capability/module lists hand-synced across crates | F2 | high | low |
| mod-dup | needless-link | codemod:* registration copied in five engines | F2 | high | low |
| native-wasm-ast | hidden-link | Native/WASM ast-grep module ids diverge | F2 | high | low |
| langdetect | hidden-link | Extension→language maps disagree by call site | F2 | high | low |
| dynlang | special-case | Dynamic language inventories triplicated | F2 | high | low |
| walker-dup | needless-link | Parallel walker enums factory ↔ javascript | F2 | high | low |
| matrix-meta | hidden-link | `_meta_` prefix convention across shard/scheduler/engine | F3 | high | medium |
| recompile-hash | hidden-link | Recompile hasher ≠ task-identity hasher (keep separate inputs) | F3 | high | medium |
| cancel-string | special-case | Cancel encoded as Failed + magic string | F3 | high | medium |
| pro-dry-run | special-case | Four flags for one preview caller | F3 | high | medium |
| global-events | hidden-link | Process-global workflow event registry | F4 | high | medium |
| parallel-log-writers | hidden-link | Task logs mutated outside TaskStateService | F4 | high | medium |
| three-run-paths | hidden-link | run / workflow run / jssg share post-run by copy | F5 | high | low |
| output-rs-misnamed | surplus | commands/output.rs is not the CLI sink | F5 | high | low |
| engine-god | needless-link | engine.rs owns schedule + every step kind | F6 | high | high |
| dual-exec-paths | hidden-link | Sync vs spawned task execution by entrypoint | F6 | high | high |
| step-executor-thin | surplus | StepExecutor is pass-through back into Engine | F6 | high | high |
| swallowed-errors | special-case | Recompile/condition errors become skip/continue | F6 | high | high |
| registry-auth-split | hidden-link | Auth/publish split across CLI and RegistryClient | F7 | high | medium |
| harness-god | surplus | harness_adapter.rs mega-module (~6852 LOC) | F8 | medium | medium |
| harness-lock-sleep | special-case | Harness lock retry via sleep special case | F8 | medium | medium |
| report-server-pile | surplus | report_server mixes HTTP/OIDC/feedback/idle | F8 | medium | medium |
| expr-in-models | needless-link | Expression eval + env coupling in models crate | F9 | medium | low |
| typeinfo-dead | surplus | type_info / get_type speculative stubs | F10 | medium | low |
| noop-unused | surplus | NoopSemanticProvider unused outside unit tests | F10 | medium | low |
| shallow-exports | needless-link | One-line node/web export shims | F10 | medium | low |
| gen-artifacts | surplus | Committed generated TS with wrong generator header | F10 | medium | low |
| factory-bypass | needless-link | Semantic fixtures bypass SemanticFactory | F10 | medium | low |
| init-lang-matrix | hidden-link | js-astgrep language matrix duplicated + silent JS fallback | F10 | medium | low |

## Method & limits

- Read crate `AGENTS.md` maps and dependency roles
- Explored workflow engine, CLI/output, and sandbox/semantic areas in parallel
- Verified key evidence (engine cancel string, quiet dual paths, LOC/churn)
- Did **not** execute the skill’s “fix everything” step
- Did not run Comments/Tests optional passes
- Churn since 2025-08-01: `engine.rs` 92 · `run.rs` 54 · `jssg/run.rs` 46 · `workflow/run.rs` 34

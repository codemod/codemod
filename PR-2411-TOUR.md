# Tour of codemod/codemod#2411 (TS workflows)

State described: `prototype/typescript-orchestration` @ 682516f0 (pushed Nov 9), against its merge base with `main` (0d66d565). 282 files, +46,369 / −1,339. Line counts are added lines from `git diff --numstat`.

## New Rust crates (the run path, no Node)

| Crate | Size | What | Why |
|---|---:|---|---|
| `orchestration-build` | +2,033 (10 files) | Builds `workflow.ts`: oxc finds each `jssg()` transform and cuts it out with its imports; rolldown bundles each into a hashed artifact, then bundles the workflow with the runtime; lowers async to generators; caches on disk by content. | So the CLI can build a workflow itself, with no Node or `node_modules`. |
| `orchestration-quickjs-host` | +2,144 (9) | Runs the bundle in one QuickJS context inside `codemod`: fs, spawn, HTTP, clock and JSSG natives; a worker pool for batches of 64+ files; signal cancellation; `--history` read/write. Embeds the TS runtime at build time. | It is the runtime. |
| `jssg-batch` | +1,146 (3) | One JSSG batch: verify the artifact hash, static selector, parse, call the transform per file, validate every path, return edits and outputs as data. | YAML's JSSG semantics in one place the host reuses, with no repo writes on this path. |
| `orchestration-walker` | +1,180 (3) | Native file selection with the YAML engine's walker rules, plus bulk source reads. | Speed. The same walker in TS inside QuickJS gives the same result, slower. |
| `orchestration-agent` | +3,050 (9) | The `codemod-agent` binary: runs one `agent()` step (built-in, Claude Code or Codex) in its own process with an allowlisted env; key on stdin, completion on stdout. | Agent tools need their own process and cwd, isolated from the CLI. |
| `bench-timings` | +1,252 (7) | Phase timings behind a `timings` cargo feature (off by default); cli, core and codemod-sandbox depend on it. | Benchmarking only. |

## Changed Rust crates

| Crate | Size | What | Why |
|---|---:|---|---|
| `cli` | +1,993 / −6 (src ~250, tests ~1,730) | `workflow run -w` (`commands/workflow/typescript.rs`, flags in `run.rs`, `main.rs`), `timings` feature, completion-signal wait in `workflow_runner.rs`. Tests: `typescript_workflow.rs`, `jssg_parity.rs` (byte parity with YAML), `typescript_timings.rs`, `workflow_run_completion.rs`, shared `tests/support`. | The command, and the end-to-end specs. |
| `codemod-sandbox` | +3,159 / −1,264 (22) | `SgNode`/`SgRoot` logic moved into engine-agnostic `sg_core.rs` (rquickjs classes are thin wrappers); `embedded.rs` runs transforms in a context the embedder owns; staged writes, selector helpers, `walk.rs`. An API snapshot test pins the `SgNode` API. | Lets the host reuse JSSG execution. Additive for YAML; the sg_core split is the biggest diff against existing code. |
| `core` | +340 / −67 (8) | `workflow_completion.rs`: in-process completion signal for YAML `workflow run`/`resume`, replacing a poll that added 1–2 s per run; timings hooks. | Needed for a fair benchmark. It's a standalone YAML fix and could be its own PR. |
| `mcp` | +1 | Follow-up to a changed signature. | |

## TS packages

| Package | Size | What | Why |
|---|---:|---|---|
| `packages/orchestration` (new) | +22,123 (150 files) | The library and runtime. `src` ~10k: authoring 1.5k (the 7 functions), core 2k (protocol, history, events, timings), execution 2.8k (jssg/shell/agent/assessment executors, decision adapters), runtime 0.6k (run, replay gate, scheduler, capabilities), host 2.8k (QuickJS entry and async context 0.9k; Node host and test harness 0.3k; dashboard model + UI 1.6k). Tests 7.9k. Docs 2.7k (README, DESIGN, RUNTIMES, PROTOCOL). Demo 1k. Fixtures 0.3k (protocol JSON shared with the Rust crates). | Authoring API and orchestration policy stay TS; it runs inside QuickJS. |
| `packages/jssg-types` | +40 | `StructuredCodemod` / `CodemodResult` types: a transform can return `{ content, output }`. | How a jssg step returns data to the next step. |

## Other

| Path | Size | What | Recommendation |
|---|---:|---|---|
| `benchmarks/runtimes/` | +7,509 (37 files) | Harness from the earlier runtime comparison: shell/C/mjs scripts (setup, run, verify, rounds, analysis), a two-step vite workflow in YAML and TS, timing fixtures, a node:test suite, and committed results (`results/phase-g/report.json` alone is 3,623 lines). Results compare ts-node, ts-bridge and ts-quickjs, two of which no longer exist. | Drop from the PR (see below). |
| `HANDOFF.md` (repo root) | +264 | Internal session hand-off notes from the prototype work. | Drop. |
| Root files | small | Cargo workspace members and lockfiles, root `package.json` `test:bench` script, `.oxfmtrc.json` ignore for bench results, `.gitignore`. | Follow whatever is dropped. |

### Does anything need `benchmarks/`?

- No CI job runs it (`.github` is unchanged by the PR).
- `crates/cli/tests/typescript_timings.rs` uses its two transforms (`benchmarks/runtimes/transforms/`). They'd move to a test fixture folder.
- Root `package.json` has `test:bench` for its node:test suite; that script would go.
- Pointers only: `packages/orchestration` README, RUNTIMES.md, `src/core/timings.ts`, `tests/timings.test.ts`, `crates/bench-timings` docs, `.oxfmtrc.json`, HANDOFF.md.
- The numbers in the PR and the RFC come from the separate proof harness outside the repo, not from this folder.

Recommendation: drop `benchmarks/` and `HANDOFF.md`, move the two transforms into `crates/cli/tests/fixtures/`, remove `test:bench` and the doc pointers, and keep the numbers plus a short repro note in the RFC. Net about −7,800 lines.

Two more candidates for the same cut, each Dev's call:
- **Dashboard** (`src/host/dashboard`, 1,604 lines, plus `tests/dashboard.test.ts`, 416): its server was removed with the Node launcher, so nothing in the CLI uses it now.
- **`bench-timings` + the `timings` feature** (1,252 lines plus hooks in cli/core/sandbox and `typescript_timings.rs`): it only serves benchmarking. Keeping it lets the perf numbers be reproduced from the repo.

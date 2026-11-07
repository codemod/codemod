# Handoff: single-runtime orchestration (Node / QuickJS) + benchmark

_Status: Phase G done (2026-11-07): F1-F3 integrated into rt/integration, F2 timings finished and verified, multi-size report recorded on a Linux VM. See section 13._
_Private repo: https://github.com/devagrawal09/codemod (private, remote `fork`) · base: codemod/codemod `prototype/typescript-orchestration` @ `6d0ad87` · main checkout never modified._

## 1. Goal
Run an entire TypeScript workflow (packages/orchestration) without a QuickJS sandbox per JSSG execution:
- `--runtime=node`: whole workflow + every transform in ONE Node process.
- `--runtime=quickjs`: whole workflow + every transform in ONE QuickJS context hosted by a Rust binary (no Node at run time).
- `--runtime=bridge` (default, unchanged): Node control plane, one `butterflow-execution-bridge` process per command, fresh QuickJS runtime per file.
Plus a benchmark comparing those against today's YAML engine (`codemod workflow run`), all producing a byte-identical diff.

## 2. Architecture
```
                       workflow.ts (author code)
                               │
                 packages/orchestration control plane (TS)
                 RunRuntime · ReplayGate · history · AdmissionScheduler · composition
                               │  Host interface (src/core/host.ts): fs, path, spawn, async-context, env, clock,
                               │  optional selectFiles / readSources (native)
        ┌──────────────────────┼───────────────────────────┐
        ▼                      ▼                           ▼
   runtime=bridge         runtime=node                runtime=quickjs
   Node host              Node host                   Rust host (crates/orchestration-quickjs-host)
   spawn bridge/cmd  ───► crates/execution-bridge     one AsyncRuntime + one AsyncContext for the run
   (QuickJS per file)     jssg-napi addon (in-proc)   codemod:ast-grep registered in the same context
        │                      │                           │
        └──────────── crates/jssg-batch (shared rules: hash check, containment, selector skip, parse, convert)
                               │
                 crates/codemod-sandbox/src/ast_grep/sg_core.rs (binding-agnostic SgRoot/SgNode)
                   ├─ sg_node.rs   rquickjs facade (bridge, quickjs host, YAML engine)
                   └─ crates/jssg-napi   napi-rs facade (node runtime)
```

### JSSG command path (all runtimes)
```
executeJssgCommand (src/execution/jssg.ts)
  ├─ selectFiles (TS, or Host.selectFiles native)      ← gitignore, include/exclude, language extensions
  ├─ read sources (Host.fs.readSources batched)
  ├─ dispatch(request) → { files: FileOutcome[] }     ← per-runtime: bridge process | in-proc napi | in-context quickjs
  │     per file: selector skip → transform(root) → classify_transform_result → FileOutcome (jssg-batch)
  ├─ validate edits, cross-file conflict check
  └─ commit (nothing written before every file succeeded)
```
Invariants kept: snapshot semantics, all-or-nothing commit, path containment checked on both sides, transform module evaluated once per run (keyed by artifact hash), BOM passed through verbatim (matches YAML engine).

## 3. Layout (new or heavily changed)
| Path | What |
|---|---|
| `crates/codemod-sandbox/src/ast_grep/sg_core.rs` | binding-agnostic AST core; `sg_node.rs` is now a thin rquickjs facade; API snapshot test in `tests/integration/sg_api/` |
| `crates/jssg-batch` | one copy of the bridge's batch rules (BatchPlan, classify_transform_result, containment); used by bridge, quickjs host, jssg-napi |
| `crates/jssg-napi` + `packages/jssg-napi` | napi-rs addon exposing SgRoot/SgNode, parse on worker threads, native selectFiles/readSources; `pnpm --filter @codemod.com/jssg-napi build` |
| `crates/orchestration-quickjs-host` | binary `orchestration-quickjs-host run <bundle> --target <dir> [--json] [--history <file>] [--timings]`; Rust-backed Host; SIGINT/SIGTERM abort |
| `packages/orchestration/src/core/{host,posix,async-context}.ts` | Host interface, pure-TS path, async-context fallback |
| `packages/orchestration/src/portable.ts` | Node-free entry (guarded by `tests/portable.test.ts`) |
| `packages/orchestration/src/host/runtimes/{types,registry,bridge,node,quickjs}.ts` | runtime providers + capabilities; `src/runtime/capabilities.ts` |
| `packages/orchestration/src/host/quickjs/` | bundle builder (`codemod-workflow build --runtime=quickjs -o`), in-context dispatch |
| `packages/orchestration/bin/{codemod-workflow,register-ts}.mjs` | single Node process, in-process type stripping, on-disk cache (`CODEMOD_WORKFLOW_CACHE`) |
| `packages/orchestration/RUNTIMES.md` | runtime design doc (read this first) |
| `benchmarks/runtimes/` | harness: setup.sh (pinned vite), verify.sh (identical-diff gate), run.sh, lib.sh, README.md with results |

## 4. Build / run / test
```bash
pnpm install
cargo build --release -p butterflow-execution-bridge -p orchestration-quickjs-host -p codemod
pnpm --filter @codemod.com/jssg-napi build          # release .node for this platform (macOS arm64 only so far)

# run a workflow
node packages/orchestration/bin/codemod-workflow.mjs run <workflow.ts> --target <repo> --runtime=bridge|node|quickjs
node packages/orchestration/bin/codemod-workflow.mjs build <workflow.ts> --runtime=quickjs -o bundle.mjs
target/release/orchestration-quickjs-host run bundle.mjs --target <repo> --timings

# tests
pnpm --filter @codemod.com/orchestration typecheck
CODEMOD_REQUIRE_RUNTIMES=1 pnpm --filter @codemod.com/orchestration test   # requires addon + host built
pnpm --filter @codemod.com/orchestration test:e2e                           # bridge + node + quickjs + parity suites
cargo test -p codemod-sandbox -p jssg-batch -p jssg-napi -p orchestration-quickjs-host -p butterflow-execution-bridge -p codemod
cargo clippy -p codemod-sandbox -p jssg-batch -p jssg-napi -p orchestration-quickjs-host -p butterflow-execution-bridge --tests --no-deps -- -D warnings
pnpm lint && pnpm format

# benchmark
bash benchmarks/runtimes/setup.sh            # clones vite v8.3.6 into ~/.cache/codemod-bench/vite (untrusted data; never run its scripts)
BENCH_WARMUP=2 BENCH_RUNS=10 bash benchmarks/runtimes/run.sh   # verify.sh gate first, then hyperfine
```

## 5. Results (phase E, rt/integration @ 316d3b8; M4 Pro 14c, hyperfine --warmup 2 --runs 10, vite v8.3.6, 331 files, diff sha256 8f7ff40f…c1c95885 in every arm)
| Arm | Wall | Phase C | Phase A | 2-file floor | CPU user+sys |
|---|---:|---:|---:|---:|---:|
| ts-node | **0.368 s** | 0.688 | — | 0.045 | 1.19 s |
| ts-quickjs | 0.703 s | 1.320 | — | 0.045 | 0.95 s |
| yaml | 1.346 s | 1.324 | 1.42 | 1.350 | 3.29 s |
| ts-bridge | 1.772 s | 1.931 | 2.49 | 0.075 | 1.78 s |

quickjs host direct on a prebuilt bundle (no Node): 0.666 s. Cold caches: quickjs 0.901 s, node 0.576 s.
**Caveat:** the yaml wall time is set by the CLI's status-poll sleeps (`crates/cli/src/workflow_runner.rs`: 1 s / 2 s between checks), not by the engine; its real time is somewhere in 0.27–1.35 s. Phase F1 fixes this with a completion signal.

## 6. Decisions (fixed)
- New crates stay outside `crates/cli`.
- One JS thread per new runtime; Rust may parse/walk/convert off-thread.
- node/quickjs support `jssg` + `shell` only; agent, assessment, semantic analysis fail with `UnsupportedInRuntimeError`.
- Transform modules evaluated once per run; module-level state unsupported (documented).
- bridge remains the default runtime.
- BOM kept verbatim everywhere (YAML engine behaviour).
- Timings: shared feature-gated crate `crates/bench-timings` (F2); yaml poll fix via engine completion signal (F1).

## 7. Phase history and branches
```
### rt/bench (1 commits, 13 files changed, 795 insertions(+))
- ab772b5 feat(bench): add runtime benchmark harness with yaml and ts-bridge arms
### rt/sg-core (2 commits, 13 files changed, 2116 insertions(+), 1075 deletions(-))
- f32c0b4 test(sandbox): assert sg_core root handles are Send + Sync; format API fixture
- dc36e2d refactor(sandbox): split SgRoot/SgNode into binding-agnostic sg_core
### rt/host-seam (3 commits, 36 files changed, 3042 insertions(+), 360 deletions(-))
- 0278c08 docs(orchestration): document runtimes, the host seam, and QuickJS host requirements
- af81081 feat(orchestration): host seam, portable entry, and runtime selection
- cc782b8 test(orchestration): copy @typesafe-ai/sdk into the installed-package e2e consumer
### rt/node (10 commits, 87 files changed, 8861 insertions(+), 1552 deletions(-))
- 21eec1d fix(orchestration): accept options before the workflow; harden and test the napi batch parser
- 2310038 feat(orchestration): node runtime over a napi-rs JSSG addon
- 0278c08 docs(orchestration): document runtimes, the host seam, and QuickJS host requirements
- af81081 feat(orchestration): host seam, portable entry, and runtime selection
- cc782b8 test(orchestration): copy @typesafe-ai/sdk into the installed-package e2e consumer
- f32c0b4 test(sandbox): assert sg_core root handles are Send + Sync; format API fixture
- ab772b5 feat(bench): add runtime benchmark harness with yaml and ts-bridge arms
- dc36e2d refactor(sandbox): split SgRoot/SgNode into binding-agnostic sg_core
### rt/quickjs (11 commits, 97 files changed, 9683 insertions(+), 1489 deletions(-))
- f95386b perf(orchestration): load esbuild and TypeScript only on a quickjs bundle cache miss
- 1dda7ac perf(orchestration): fast-path relative() in the file walker; update runtime tests for quickjs
- 08295d5 feat(orchestration): quickjs runtime: whole workflow in one QuickJS context hosted by Rust
- 0278c08 docs(orchestration): document runtimes, the host seam, and QuickJS host requirements
- af81081 feat(orchestration): host seam, portable entry, and runtime selection
- cc782b8 test(orchestration): copy @typesafe-ai/sdk into the installed-package e2e consumer
- f32c0b4 test(sandbox): assert sg_core root handles are Send + Sync; format API fixture
- ab772b5 feat(bench): add runtime benchmark harness with yaml and ts-bridge arms
### rt/d-convert (17 commits, 126 files changed, 14338 insertions(+), 2039 deletions(-))
- a52c839 perf(orchestration): keep the shared JSSG batch rules off the critical path
- 0e008b6 refactor(orchestration): share the bridge's JSSG batch rules with the node runtime
- 866153c docs(benchmarks): final four-arm runtime benchmark results
- f95386b perf(orchestration): load esbuild and TypeScript only on a quickjs bundle cache miss
- 21eec1d fix(orchestration): accept options before the workflow; harden and test the napi batch parser
- 1dda7ac perf(orchestration): fast-path relative() in the file walker; update runtime tests for quickjs
- 08295d5 feat(orchestration): quickjs runtime: whole workflow in one QuickJS context hosted by Rust
- 2310038 feat(orchestration): node runtime over a napi-rs JSSG addon
### rt/d-walker (18 commits, 125 files changed, 15100 insertions(+), 1617 deletions(-))
- 1b241e0 feat(orchestration): native file selection for the node runtime via the napi addon
- b3aeb76 perf(orchestration): faster native walk, read timings, deterministic readdir order
- e390368 feat(orchestration): native file selection and batched source reads for the quickjs host
- 866153c docs(benchmarks): final four-arm runtime benchmark results
- f95386b perf(orchestration): load esbuild and TypeScript only on a quickjs bundle cache miss
- 21eec1d fix(orchestration): accept options before the workflow; harden and test the napi batch parser
- 1dda7ac perf(orchestration): fast-path relative() in the file walker; update runtime tests for quickjs
- 08295d5 feat(orchestration): quickjs runtime: whole workflow in one QuickJS context hosted by Rust
### rt/d-startup (17 commits, 124 files changed, 13480 insertions(+), 1744 deletions(-))
- df30be7 perf(orchestration): load runtime executors and the dashboard on demand; cache-safe TypeScript format detection
- fbcb469 perf(orchestration): run the CLI in one Node process with cached type stripping and lazy TypeScript/esbuild
- 866153c docs(benchmarks): final four-arm runtime benchmark results
- f95386b perf(orchestration): load esbuild and TypeScript only on a quickjs bundle cache miss
- 21eec1d fix(orchestration): accept options before the workflow; harden and test the napi batch parser
- 1dda7ac perf(orchestration): fast-path relative() in the file walker; update runtime tests for quickjs
- 08295d5 feat(orchestration): quickjs runtime: whole workflow in one QuickJS context hosted by Rust
- 2310038 feat(orchestration): node runtime over a napi-rs JSSG addon
### rt/integration (28 commits incl. merges, 139 files changed, 17913 insertions(+), 2196 deletions(-))
- ffb3a61 Merge branch 'rt/d-startup' into rt/integration
- ee4bb5a Merge branch 'rt/d-walker' into rt/integration
- fea13f3 Merge branch 'rt/d-convert' into rt/integration
- f8ae6be Merge branch 'rt/quickjs' into rt/integration
- 99ea04a Merge branch 'rt/bench' into rt/integration
- 9656453 Merge branch 'rt/sg-core' into rt/integration
- 316d3b8 docs(benchmarks): final four-arm results after phase D, BENCH_TIMEOUT knob
- a3aa624 fix(orchestration): let a SIGINT cancel a running quickjs JSSG batch
- 3fefc73 fix(orchestration): hand JSSG sources to every runtime verbatim, BOM included
- 866153c docs(benchmarks): final four-arm runtime benchmark results
- 21eec1d fix(orchestration): accept options before the workflow; harden and test the napi batch parser

Note: branch counts are cumulative (each branch includes the commits of the branches it was based on). Phase E on rt/integration also contains: 3fefc73 (verbatim sources, BOM), a3aa624 (SIGINT cancels a running quickjs batch), 316d3b8 (final results, BENCH_TIMEOUT).

Private mirror branches (all pushed as-is, unsquashed, by user choice): prototype/typescript-orchestration, rt/integration, rt/bench, rt/sg-core, rt/host-seam, rt/node, rt/quickjs, rt/d-convert, rt/d-walker, rt/d-startup, rt/f-signal, rt/f-timings, rt/f-harness.
```

## 8. Phase F (in flight at pause)
### F1 — completion signal (rt/f-signal, DONE, verified)
Commits: `ca8bb5b` feat(core): signal in-process workflow executor exits per run · `013964f` fix(cli): exit workflow run/resume as soon as the executor finishes (6 files, +547/−7).
- `crates/core/src/workflow_completion.rs` (new) + `engine.rs`: every executor start registers per run id; exit (success/error/panic) wakes waiters. `Engine::completion_signal(run_id)` → Some for runs started in-process, None for runs driven elsewhere (cloud state) which keep plain polling.
- `crates/cli/src/workflow_runner.rs` + `commands/workflow/resume.rs`: the wait loop races the signal against the old 1 s/2 s sleep; stored status stays the source of truth. Behaviour change: an executor that errors without recording a final status now fails the CLI instead of polling forever.
- Verification: cargo fmt/clippy clean; `cargo test -p butterflow-core` (3 new tests) and `-p codemod` (456 unit + integration incl. new `tests/workflow_run_completion.rs`) pass.
- Numbers (release, hyperfine --warmup 2 --runs 10): yaml 2-file floor **1.446 → 0.464 s**; yaml on vite **1.359 → 1.087 s** (0.93–1.35 spread, now engine variance not sleep). Remaining floor ≈ 351 ms is CLI startup before "Workflow started". Single gate pass: yaml 0.75 s, ts-bridge 1.81, ts-node 0.37, ts-quickjs 0.71.

### F2 — zero-overhead timings (rt/f-timings, WIP at pause; finished in Phase G, see section 13)
Commit: `abb52b5` wip(timings): feature-gated bench-timings crate and Rust-side spans (INCOMPLETE) — 45 files, +1,614/−318. Subagent was killed by a server restart mid-task; committed as-is.
- Present: `crates/bench-timings` (feature-gated), `timings` feature wiring in core/cli/codemod-sandbox/execution-bridge/jssg-napi/orchestration-quickjs-host Cargo.toml, spans/marks in those crates, started `packages/orchestration/src/core/timings.ts` and `src/host/timings-file.ts`.
- `cargo check -p butterflow-core -p codemod -p jssg-napi -p orchestration-quickjs-host -p butterflow-execution-bridge` passed at commit time (default features; the `timings` feature itself is unverified).
- Missing: JS-side spans for node/quickjs dispatch, child-file registration from the Node CLI, overhead proof (≤1 % on/off), coverage check (≥95 % of wall), tests, fmt/clippy.

### F3 — robust harness (rt/f-harness, DONE, no final report received)
Commits: `bf7bf8b` interleaved rounds harness with bench-exec launcher, preflight, sizes, analysis, compare, overhead · `facd20c` per-size inputs, start-of-run worktree state in provenance, README (26 files, +2,573/−20).
- New under `benchmarks/runtimes/`: `bench-exec` launcher (BENCH_T0_NS), `preflight.sh`, `rounds.sh` (shuffled arms, hyperfine -N, one run per arm per round, diff check after every run), `sizes.sh` (probe / vite / vite×4 / vite×16), `analyze.mjs`/`compare.mjs`/`overhead.{sh,mjs}`/`provenance.mjs`, timings fixtures per arm, `tests/analysis.test.mjs` (451 lines). README documents each script.
- Not confirmed (subagent interrupted before reporting): the end-to-end R=3 run on probe+vite and the analysis test run. Run `node --test benchmarks/runtimes/tests/` and `bash benchmarks/runtimes/sizes.sh` first when resuming.

## 9. Open issues
- No resume from `codemod-workflow run` (only the quickjs host binary has `--history`).
- On-disk build cache misses import-resolution-only changes (package.json `exports`, tsconfig, shadowing files): delete `CODEMOD_WORKFLOW_CACHE` to rebuild.
- Cache-root logic duplicated in `bin/register-ts.mjs` and `src/bundle/cache.ts` (kept in sync by hand).
- napi addon built for macOS arm64 only; no per-platform packaging.
- Dashboard (`--dashboard`) unsupported under quickjs.
- One early ~1 h quickjs hang never reproduced (140-run stress clean); SIGINT-ignored bug found and fixed instead.
- Subagent-driven work: three Claude Code sessions stalled silently once each (phases A and F); detect via no file activity + no processes, resume by continuing the session.

## 10. How Phase G was planned (done; see section 13)
1. Merge `rt/f-signal`, `rt/f-timings`, `rt/f-harness` into `rt/integration` (expect conflicts in `crates/cli/src/workflow_runner.rs` and `crates/core/src/engine.rs` between F1 and F2 marks; `benchmarks/runtimes/lib.sh` between F3 and F2's env plumbing).
2. Overhead proof: each arm with `CODEMOD_TIMINGS_FILE` unset vs set, ratio within 1 %; feature-off vs feature-on builds.
3. `benchmarks/runtimes/preflight.sh && sizes.sh` (probe / vite / vite×4 / vite×16), `analyze.mjs` → report.md with medians, IQR, bootstrap CIs, fixed-vs-per-file fit, per-phase table.
4. Re-run verify.sh gate for all four arms; update README results; squash per the one-commit-per-phase rule; push.

## 11. Local state
- Dashboard: `/Users/devagr/codemod/rt-dashboard/dashboard.html` (regenerated every 10 min by the `rt-dashboard` event; facts in `status.json`).
- Worktrees kept: `codemod-rt-integration` (29 G of cargo targets; `cargo clean` when done), `codemod-rt-f-{signal,timings,harness}`.
- Bench clones: `~/.cache/codemod-bench/vite-final2`, `vite-integration-stress`.

## 12. Incident: stray PRs on codemod/codemod (2026-11-07 06:19–06:23 UTC)
Two PRs titled "Shell Node" (#2472 head codemod-c4ee35be, #2473 head codemod-ce4907ec, base main) appeared on codemod/codemod under devagrawal09 while the Phase F subagents ran. No command in this project pushed to origin or created PRs; all pushes used the `fork` remote. The same checkpoint automation leaves local `codemod-<hash>` branches in the shared repo (11 exist). Both PRs were closed and their upstream branches deleted; origin has no codemod-* branches left. The automation's identity is recorded in the dashboard status file once found; until it is disabled, assume any agent run in this repo may push checkpoint branches upstream.

## 13. Phase G (done, 2026-11-07, Linux cloud VM)

Commits on rt/integration (on top of `8a7c8f3`): `4cbbd37` / `380faa7` / `e6224e2` merges of rt/f-signal, rt/f-harness, rt/f-timings (one conflict in `crates/core/src/engine.rs`: the F1 executor guard kept, F2's `engine_done` mark added); `316e40d` feat(timings): finish phase timings, full coverage, overhead tooling; `7266b91` drop profile-node.mjs and the unused Rust bench-exec; `e2ce57f` preflight `ps` fix; then the results commit (README, `benchmarks/runtimes/results/phase-g/`, this file). F2 was not squashed: it is already merged and pushed, and rewriting it would need a force push.

### What F2 needed
- Node CLI wiring: `bin/codemod-workflow.mjs` installs `host/timings-file.ts` only when `CODEMOD_TIMINGS_FILE` is set; arm `ts-<runtime>`; `boot` until `main`, `plan` for workflow load/launch/printing. `execution/child-timings.ts` times bridge and quickjs-host children (registers them, sets `BENCH_T0_NS`, charges spawn-to-close minus the child's wall to `setup`).
- Merge rule fixed (`crates/bench-timings`): per-thread wall is now **summed** (max-over-threads undercounted async code hopping tokio threads and sequential worker pools). Worker pools run in `bench_timings::parallel(threads, f)` regions (owner records total / pool size as wall, total as cpu). New `Span` (pausable) and `Residual` (scope minus nested spans on the thread). Tests: `tests/regions.rs`, updated `report.rs`/`process.rs`.
- Gaps closed: QuickJS context/runtime teardown (sandbox + quickjs host) as `setup`; yaml engine scheduling (`Span` in `execute_workflow`/`execute_task`) and pre-run dirty check as `plan`, step preparation as `setup`, walker traversal as `select`, per-file engine glue as `setup` (`Residual`); JS batch loop as back-to-back laps (chunk waits = `parse`, `finish` = `convert`), with the nested Rust convert/parse spans removed (napi, quickjs host); result validation in `jssg.ts` = `conflict`.
- Removed: `benchmarks/runtimes/profile-node.mjs` (old `profile` hooks are gone), `crates/bench-timings/src/bin/bench-exec.rs` (the harness uses `bench-exec.c`).
- Harness: `overhead.sh` build mode (`BENCH_OVERHEAD_BASELINE=<target dir without the feature>`), paired bootstrap in `overhead.mjs`/`analysis/stats.mjs` (verdict from the paired CI) + test; preflight tolerates BusyBox `ps`.
- Tests: `packages/orchestration/tests/timings.test.ts` (unit), `tests/timings.e2e.test.ts` + `pnpm test:e2e:timings` (all runtimes write files, children listed, merged phases ≤ wall and > 80 %).

### Verification
- Coverage (merged phases / in-process wall): 96.4-100 % for every arm at vite, vite-x4, vite-x16 (target ≥ 95 %); 94-96 % on the 2-file probe.
- Overhead: 20 interleaved pairs per arm, env mode and build mode: paired ratios 0.97-1.01, all CIs contain 1.0; VM noise (±2-3 % CI) only proves ≤ 1 % for ts-bridge env mode. Measured per-span costs bound it at ≤ 0.33 % on, < 0.01 % compiled-in-off (README "Results").
- Gate: identical diffs for all four arms at every size (vite `8f7ff40f…c1c95885`, same as macOS).
- Tests: orchestration vitest (unit) 488 + e2e 47 + timings e2e 4 pass; analysis tests 22 pass; cargo tests 921 pass across bench-timings, codemod-sandbox, jssg-batch, jssg-napi, orchestration-quickjs-host, execution-bridge, butterflow-core, codemod. Two pass alone but flaked once in the full parallel run: `butterflow-core engine_tests::test_trigger_all` / `test_run_script_preserves_platform_inherited_llm_env` (sleep/env-var sensitive) and `execution-bridge jssg::setup_and_transform_failures_fail_the_batch_before_any_result` (tokio "drop a runtime in async context"; 0/180 on reruns incl. 4-way parallel). Core git tests need `TEMBO_ALLOW_GIT_CONFIG=1 TEMBO_BYPASS_GIT_RESTRICTIONS=1` in this sandbox (its git wrapper blocks `git config`/`commit` in scratch repos; the bench scripts need the same).
- clippy `-D warnings` clean on the touched crates with and without `timings`, except two lints new in rustc 1.96 (the sandbox's toolchain; the repo pins 1.94.1) in untouched files: `codemod-sandbox/src/ast_grep/scanner.rs:70` (unnecessary_sort_by), `cli/src/tui/mod.rs:744` (collapsible_match).

### Results (vite-x16, median of 10)
ts-node 13.36 s, ts-quickjs 20.10 s (1.50x), yaml 25.79 s (1.93x), ts-bridge 65.60 s (4.91x); per file 0.51 / 0.77 / 1.00 / 2.54 ms. Details: `benchmarks/runtimes/results/phase-g/report.md`.

### Open issues found in Phase G
- yaml `plan` grows superlinearly (163 ms vite → 747 ms x4 → 5,917 ms x16): engine scheduling/state bookkeeping around steps; profile it.
- ts-quickjs parses on the JS thread (699 ms vs ts-node's 289 ms blocked on vite); moving parse off-thread in the host is the obvious next win.
- napi addon now also builds on linux-x64 (`jssg-napi.linux-x64.node`, local build only; still no packaging).
- Checkpoint automation struck again: a "Node 2" commit and `codemod-*` local branches appeared mid-session, and one commit landed on a pushed `codemod-9e4fa9a2` branch of the fork; it was re-committed onto rt/integration and the stray remote branch deleted. Nothing reached codemod/codemod.

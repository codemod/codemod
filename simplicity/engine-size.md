# Engine size: YAML (butterflow, `main`) vs TypeScript orchestration

- **YAML:** `codemod/codemod` `main` @ `0d66d565`. Crates `models`, `scheduler`, `state`, `runners`, `core`, plus the workflow parts of `crates/cli`.
- **TS:** `prototype/typescript-orchestration` @ `72865ebd`. `packages/orchestration`, `packages/jssg-napi`, and the crates `jssg-batch`, `jssg-napi`, `orchestration-walker`, `orchestration-agent`, `orchestration-quickjs-host` and `bench-timings`.
- **Not counted for either engine:** `codemod-sandbox` (JSSG/QuickJS + ast-grep bindings), `language-*`, `semantic-factory`, `ai` (`codemod-ai`), `llrt-capabilities` and `telemetry`. Both engines use them.
- **What a line is:** a line with code after comments and blank lines are removed. Tests don't count: `tests/` directories and every `#[cfg(test)]` item are left out. A small lexer in `count-loc.py` strips comments and string contents, then matches braces to find the test blocks.
- **How to audit it:** every file and every line range is listed in `engine-size.csv`, with its category and feature. No file in the scanned source folders is left unassigned (the script checks this).

## Headline

| Category | YAML | TS | TS vs YAML |
|---|---:|---:|---:|
| **(a) Core orchestration**: model, scheduling, state/history, step dispatch, params/input, CLI entry | **6,491** | **3,843** | **−41%** |
| (a′) Step executors both engines have (shell, agent/AI) | 1,879 | 1,642 | −13% |
| **(b) JSSG/runtime glue** (default runtime only) | 1,937 | 3,853 | **+99%** |
| **Comparable total (a + a′ + b)** | **10,307** | **9,338** | **−9%** |
| (c) Features only one engine has | 13,038 | 6,022 | n/a |
| All counted code | 23,345 | 15,360 | n/a |
| Test code (not included above): `tests/` dirs + `cfg(test)` blocks | 8,715 + 9,173 | 11,294 + 847 | n/a |

What the numbers mean:
- **The orchestration core shrinks by 41%** (6.5k → 3.8k lines).
- **The JSSG glue doubles.** The TS engine ships its own Rust batch rules (`jssg-batch`), a native walker (`orchestration-walker`), a napi addon (Rust + JS), and a build-time bundler for inline transforms. The YAML engine gets the same work from code inside `codemod-sandbox`/`core`, which is left out of the count above.
- So the apples-to-apples total is only 9% smaller. **Simplicity is the stronger case for authoring and for the core, not for total size.**

## (a) Core orchestration, by feature

| YAML | lines | TS | lines |
|---|---:|---|---:|
| workflow model + `${{ }}` templating (`crates/models`, 13 files) | 1,167 | authoring model (`authoring/*`: runnables, command, composition, dynamic, schema, target) | 1,048 |
| engine core: `engine.rs` struct/ctor, task spawn, run/cancel/status, `execute_workflow`, `recompile_matrix_tasks`, `execute_task`, prepare/finalize step; plus `step_executor`, `task_state_service`, `utils`, `config`, `workflow_runtime` events/session | 3,447 | core contracts (protocol v8, history, events, errors, host seam, posix path) | 966 |
| | | run/replay (`runtime/*`, `operation-executor`) | 464 |
| scheduler (DAG/matrix) | 413 | admission scheduler | 259 |
| local state (`state/lib.rs`, `local_adapter.rs`) | 317 | runtime registry + node provider | 313 |
| CLI: `workflow run`/`validate`, `workflow_runner.rs` run+wait, `cli/engine.rs` | 1,147 | CLI + Node host (`host/cli.ts`, `node-host.ts`, `bin/*`, `index.ts`, `portable.ts`) | 793 |
| **total** | **6,491** | **total** | **3,843** |

`crates/core/src/engine.rs` is 5,248 lines, so it was split by function. The ranges are listed in the CSV: 3,714 code lines in total: 1,595 go to (a), 696 to agent/shell steps, 301 to JSSG glue, and 1,122 to (c) features.

## (a′) and (b) by feature

| | YAML | | TS | |
|---|---|---:|---|---:|
| shell step | `runners` direct runner + `execute_run_script_step` | 394 | `execution/shell.ts`, `process-tree.ts` | 213 |
| agent/AI step | `execute_ai_step`, `launch_agent`, `ai_agent_stream`, `llm_usage`, `ai_handoff.rs:1-332` | 1,485 | `execution/agent*.ts`, `agent-bin.ts`, `crates/orchestration-agent` | 1,429 |
| JSSG glue | `jssg_execution_service`, `execution`, `file_ops`, `execution_stats`, JSSG parts of `engine.rs` | 1,937 | `execution/jssg*.ts`, `files.ts`, `node-jssg.ts`, `transform.ts` (TS types) | 758 |
| | | | bundler: `bundle/build.ts` + `cache.ts` (on every run's path, so counted here) | 565 |
| | | | `crates/jssg-batch` | 483 |
| | | | napi addon: Rust 976 + JS package 246 | 1,222 |
| | | | `crates/orchestration-walker` | 825 |

## (c) Features only one engine has (not counted as bloat)

| YAML-only feature | lines | TS-only feature | lines |
|---|---:|---|---:|
| TUI (`cli/src/tui/*`, `workflow tui`) | 2,758 | quickjs runtime: sandboxed alternative (`host/quickjs/*`, `quickjs-bundle.ts`, `crates/orchestration-quickjs-host` except `http.rs`) | 2,155 |
| registry + nested codemod steps (`registry*.rs`, `nested_codemod_*`, `codemod run`, registry client) | 2,200 | local dashboard (`host/dashboard/*`, incl. 1,172-line `ui.html`) | 2,153 |
| reports / metrics (`report.rs`, `report_server.rs`) | 1,464 | benchmark timings (`bench-timings` crate, `timings*.ts`) | 872 |
| git ops / managed git / PRs / dirty-git check | 1,302 | assessment step (TypeSafe), incl. quickjs `fetch`/`http.rs` | 842 |
| sharding (`shard.rs` + shard step) | 861 | | |
| AI agent UX (agent log renderer, agent picker) | 723 | | |
| structured logging (`--format jsonl`, stdout capture)¹ | 504 | | |
| cloud state adapter | 466 | | |
| progress bar UI | 373 | | |
| interactive approvals (shell/PR/capabilities/dirty-git/agent/selection prompts) | 332 | | |
| AI handoff to parent coding agent (`ai_handoff.rs:333-end`, `emit_ai_instructions`) | 331 | | |
| run management (`workflow status/list/cancel`) | 308 | | |
| resume (`resume_workflow`, `workflow resume`) | 296 | | |
| Windows `cmd` batch runner (`cmd_script.rs`) | 288 | | |
| ast-grep YAML-rule step | 260 | | |
| dry-run diff output | 188 | | |
| capabilities (LLRT module permissions + prompt) | 125 | | |
| container runners (docker/podman) | 123 | | |
| manual nodes / triggers | 114 | | |
| install-skill step (engine part only) | 22 | | |
| **total** | **13,038** | **total** | **6,022** |

¹ `structured_log.rs` is partly core step logging. If you count it as core, YAML (a) becomes 6,995 and the core reduction is −45%.

Left out of the counts:
- Test support: YAML `state/mock_adapter.rs` (171 lines) and TS `host/harness.ts` (136 lines).
- Shared sandbox changes on the branch. `sg_core.rs` splits `sg_node.rs`, which adds about 140 net lines and is used by both engines. `sandbox/engine/embedded.rs` (166 lines) is used only by the quickjs runtime.

## Dependencies and module counts

| | YAML | TS |
|---|---|---|
| Rust crates | 5 (`models`, `scheduler`, `state`, `runners`, `core`) + CLI modules | 6 (`jssg-batch`, `jssg-napi`, `orchestration-walker`, `orchestration-agent`, `orchestration-quickjs-host`, `bench-timings`); 4 on the default node path |
| npm packages | 0 (types only: `jssg-types`) | 2 (`@codemod.com/orchestration`, `@codemod.com/jssg-napi`) |
| Source files counted (a+b+c) | 71 | 93 |
| Direct external Cargo deps (union) | **35**: anyhow, ast-grep-config, async-trait, base64, bytes, chrono, dirs, evalexpr, flate2, futures-util, ignore, js-sys, libc, log, rand, rayon, regex, reqwest, schemars, semver, serde, serde-wasm-bindgen, serde_json, serde_yaml, sha2, similar, tar, tempfile, thiserror, tokio, ts-rs, uuid, walkdir, wasm-bindgen, wasm-bindgen-futures | **15**: ast-grep-config, ast-grep-core, libc, napi, napi-derive, regex, regress, reqwest, rquickjs, serde, serde_json, sha2, tempfile, tokio, uuid. **12** on the node path (no rquickjs, reqwest, uuid) |
| Internal workspace deps | 9 (incl. `codemod-sandbox`, `codemod-ai`, `semantic-factory`, `language-core`, `llrt-capabilities`) | 7 (incl. `codemod-sandbox`, `codemod-ai`, `semantic-factory`, `language-core`) |
| CLI-only deps tied to YAML features | ratatui, crossterm, inquire, indicatif, hyper/hyper-rustls (TUI, prompts, progress, report server: `crates/cli/Cargo.toml:51-78`) | none |
| npm runtime deps | n/a | 4 external: `typescript`, `esbuild` (both loaded only on a build-cache miss), `ignore`, `@typesafe-ai/sdk` |
| Runtime requirement | the single `codemod` binary | Node ≥ 22.15 (`bin/register-ts.mjs:18`; root `package.json` engines says ≥ 24) + `node_modules` + a per-platform `.node` addon |
| Transitive Cargo closure (Cargo.lock, all targets) | 750 crates; 311 if the shared crates' subtrees are left out | 748; 327 without shared subtrees (all crates), 181 on the node path |

Both closures are dominated by `codemod-sandbox` and `codemod-ai`, so the transitive dependency count is a wash. The difference is in direct deps and in **what you ship**: one Rust binary, versus Node plus npm packages plus a native addon per platform.

## Reproduce

```bash
python3 count-loc.py /home/user/codemod /home/user/ts-orch engine-size.csv   # prints category + feature totals
```

The dependency unions come from each crate's `[dependencies]` and `[target.*.dependencies]` sections, read with `tomllib`. The closures come from walking `Cargo.lock`, starting at those direct deps.

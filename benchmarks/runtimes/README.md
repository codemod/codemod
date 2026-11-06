# Runtime benchmark: YAML engine vs TypeScript workflow runtimes

Wall-clock comparison of the same two-step JSSG codemod run four ways on a
pinned checkout of [vitejs/vite](https://github.com/vitejs/vite), gated by a
byte-identical-diff correctness check.

## What is measured

One workflow, two JSSG steps run in order over every
`**/*.{ts,mts,cts,js,mjs,cjs}` file (language `typescript`, excluding
`**/node_modules/**`):

1. `transforms/console-to-logger.ts`: `console.log(...)` becomes `logger.info(...)`
   (only the callee is replaced, arguments keep their text).
2. `transforms/rename-vitest-import.ts`: the module specifier of named imports
   from `vitest` (`import { ... } from 'vitest'`, `import type { ... }`) becomes
   `@acme/test`, quote style kept. `vitest` is the most-imported named-import
   specifier in the checkout (about 190 files across `packages/*`, `playground/*`,
   and scripts). Subpaths like `vitest/config` are left alone.

Both transforms are plain `(root) => string | null` functions using the
`codemod:ast-grep` API (type-only imports). They use no selector, no semantic
analysis, and no module-level state, so every selected file goes through the
transform in every arm. On vite v8.3.6 the result is 331 modified files
(204 `console.log` rewrites, 196 import rewrites).

| Arm | Definition | Command |
| --- | --- | --- |
| `yaml` | `yaml/workflow.yaml` (two `js-ast-grep` steps) | release `codemod workflow run --no-interactive --disable-analytics` |
| `ts-bridge` | `ts/workflow.ts` (`sequence()` of two `jssg()` steps) | `node packages/orchestration/bin/codemod-workflow.mjs` with the release `butterflow-execution-bridge` |
| `ts-node` | same | `codemod-workflow run --runtime=node` |
| `ts-quickjs` | same | `codemod-workflow run --runtime=quickjs` |

Each arm uses its default (natural) parallelism. The harness prints it but
never sets it:

- `yaml`: steps run one after another. Each step walks and transforms files on
  `min(available_parallelism, 12)` threads (no `max_threads`), with a fresh
  QuickJS runtime per file.
- `ts-bridge`: `sequence()` runs one command at a time. Each jssg command is
  one bridge process that transforms its whole batch serially on one thread,
  with a fresh QuickJS runtime per file. File selection, reads, conflict
  checks, and writes happen in Node.
- `ts-node`: one Node process. One JS thread runs the workflow and every
  transform, serially per file. The napi addon parses and selector-matches
  64-file chunks on worker threads, one chunk ahead of the transforms.
- `ts-quickjs`: Node builds the bundle, or reuses it from its content-hash
  cache, then spawns `orchestration-quickjs-host`. One QuickJS context runs
  the control plane, the workflow, and every transform on one thread,
  serially per file. Rust parses each file on that same thread.

`ts-node` and `ts-quickjs` are skipped, with a message, when their addon or
host binary is not built. Every arm is probed first by running it on a two-file
scratch repository and checking the exact expected edit. `ts-bridge` uses
`codemod-workflow run --runtime=bridge` when the CLI accepts that form and the
legacy form otherwise. To override an arm's command line, set
`BENCH_CMD_<ARM>=...` (see `lib.sh`).

## Correctness gate

`verify.sh` runs each available arm once on a clean checkout, stages
everything (`git add -A`, so new and deleted files count too), hashes
`git diff --cached --binary`, and requires every hash to be identical. It
prints per-arm changed-file counts and timings. On a mismatch it prints the
differing `(status, resulting blob, path)` entries and exits 1. It also fails
if an arm changes nothing. `run.sh` always runs it first and benchmarks only
the arms it verified.

File selection is the same in both engines because both walkers have the
engine's semantics (`packages/orchestration/src/execution/files.ts`, pinned by
`fixtures/walker/cases.json`). The globs are written out explicitly in both
definitions, and the brace glob is expanded identically. JSSG snapshot
semantics (TS) and the engine's incremental per-file writes differ in
general, but not here: each transform reads only its own file, and the two
steps run strictly in sequence in every arm.

## The pinned input

- vitejs/vite tag `v8.3.6`, commit `0fd9a29f57d49d496f39c952f44c1b5fc3e90769`
  (pinned in `lib.sh`; `setup.sh` refuses a tag that resolves to any other commit).
- 2925 tracked files, 1621 of them matching the include globs.

The clone is untrusted data. `setup.sh` clones it into a fresh, empty
directory outside this repository with hooks, LFS/filters, and submodules
disabled. The harness never runs its scripts, installs its dependencies, or
executes tooling from inside it. It only passes the path to the arms and
resets and cleans it with git.

## How to run

```sh
# once: JS deps, the report UI the CLI embeds, and release binaries
pnpm install
pnpm --filter codemod-report-ui build          # crates/cli embeds report-ui/dist/index.html
cargo build --release -p codemod -p butterflow-execution-bridge -p orchestration-quickjs-host
pnpm --filter @codemod.com/jssg-napi build      # release napi addon for ts-node
brew install hyperfine                          # or: cargo install hyperfine
# ts-quickjs picks the newer of target/{release,debug}/orchestration-quickjs-host;
# pin it with CODEMOD_QUICKJS_HOST_BIN=target/release/orchestration-quickjs-host.

benchmarks/runtimes/setup.sh                    # clone to $HOME/.cache/codemod-bench/vite
benchmarks/runtimes/verify.sh                   # correctness gate only
benchmarks/runtimes/run.sh                      # gate + hyperfine
```

Knobs (environment): `BENCH_REPO` (checkout path; `setup.sh` also takes it as
its argument), `BENCH_RUNS` (default 5), `BENCH_WARMUP` (default 1),
`BENCH_ARMS`, `BENCH_REQUIRE_ARMS` (default `yaml ts-bridge`, which fail
instead of being skipped), `CARGO_TARGET_DIR` / `CODEMOD_BIN` /
`CODEMOD_BRIDGE_BIN`, `BENCH_CMD_<ARM>`, and `BENCH_HOME`. `BENCH_HOME` runs
every arm with `HOME` set to a scratch directory, which isolates the engine's
run state in `~/Library/Application Support/butterflow` and the global git
excludes both walkers honour.

hyperfine runs with `--warmup 1 --runs 5 --shell=bash` and
`--prepare 'git -C "$REPO" reset -q --hard && git -C "$REPO" clean -fdq'` by
default (the results below used `BENCH_WARMUP=2 BENCH_RUNS=10`).
Outputs go to `results/`:

- `benchmark.md` (committed)
- `benchmark.json` (git-ignored)
- `environment.md` (machine, toolchain, the exact commands and concurrency)
- `verify/<arm>.{diff,files,log}` (git-ignored)

`ts/workflow.ts` imports `@codemod.com/orchestration` by package name through
a git-ignored `ts/node_modules` symlink, which `lib.sh` creates.
`yaml/transforms` is a committed symlink to `../transforms`, because a
`js_file` must be a relative path inside the workflow directory.

## Results

All four arms were measured in one session on an otherwise idle machine
(no other builds or benchmarks; load average 2.0 at the start).

- Machine: MacBook Pro `Mac16,7`, Apple M4 Pro, 14 cores (10 performance + 4
  efficiency), 48 GB RAM, macOS 26.5.2 (25F84), arm64.
- Toolchain: node v24.18.0, rustc 1.94.1, hyperfine 1.20.0.
- Build: codemod 1.13.18, release builds of `rt/integration` at `f8ae6be` in
  one `CARGO_TARGET_DIR`: `codemod`, `butterflow-execution-bridge`,
  `orchestration-quickjs-host`, and the release `jssg-napi` addon.
- Input: a fresh vite v8.3.6 checkout (`BENCH_REPO=~/.cache/codemod-bench/vite-final`).
- Recorded 2026-11-06 with `BENCH_WARMUP=2 BENCH_RUNS=10 run.sh`.
- Correctness gate: all four arms gave 331 files and diff sha256
  `8f7ff40f81b3063b7594dc9d78ce1d290e1dda34bfc389edeb3f8d01c1c95885`. The
  direct host run in (a) below gave the same diff.

| Arm | Mean [s] | Min [s] | Max [s] | User [s] | System [s] | Relative |
|:---|---:|---:|---:|---:|---:|---:|
| `ts-node` | 0.688 ± 0.003 | 0.686 | 0.692 | 1.147 | 0.304 | 1.00 |
| `ts-quickjs` | 1.320 ± 0.012 | 1.307 | 1.346 | 1.276 | 0.181 | 1.92 ± 0.02 |
| `yaml` | 1.324 ± 0.003 | 1.322 | 1.331 | 2.655 | 0.636 | 1.92 ± 0.01 |
| `ts-bridge` | 1.931 ± 0.011 | 1.915 | 1.944 | 1.868 | 0.238 | 2.81 ± 0.02 |

Extra measurements, from the same session and build, also with
`--warmup 2 --runs 10`. They are not part of `run.sh`:

| Measurement | Mean [s] | Min [s] | Max [s] | User [s] | System [s] |
|:---|---:|---:|---:|---:|---:|
| (a) `orchestration-quickjs-host run <prebuilt bundle> --target <vite>` (no Node) | 1.180 ± 0.006 | 1.174 | 1.194 | 1.018 | 0.160 |
| (b) `ts-quickjs` with a cold bundle cache (`CODEMOD_WORKFLOW_CACHE` wiped before every run) | 1.479 ± 0.005 | 1.471 | 1.489 | 1.451 | 0.218 |

(c) Fixed floor per arm on the two-file probe repository (`make_probe_repo`
in `lib.sh`):

| Arm | Mean [s] | User [s] | System [s] |
|:---|---:|---:|---:|
| `yaml` | 1.341 ± 0.003 | 0.225 | 0.052 |
| `ts-bridge` | 0.309 ± 0.011 | 0.418 | 0.039 |
| `ts-node` | 0.275 ± 0.002 | 0.403 | 0.029 |
| `ts-quickjs` | 0.150 ± 0.003 | 0.266 | 0.021 |
| quickjs host on a prebuilt bundle | 0.014 ± 0.000 | 0.011 | 0.002 |

### Where the time goes

`ts-node`, in-process timers from `profile-node.mjs`, median of 3 runs on
vite. The 0.69 s wall time also includes the second Node startup
(`bin/codemod-workflow.mjs` re-spawns Node with TypeScript support):

| Phase | ms |
|:---|---:|
| import orchestration (module graph, TypeScript transform) | ~182 |
| load workflow + build the two artifacts (esbuild) | ~27 |
| file selection (walker, both commands) | ~176 |
| parse (napi, worker threads; overlaps transforms) | ~210 (of which ~80 blocks the JS thread) |
| transforms (JS thread) | ~120 |
| stage + commit | ~21 |

`ts-quickjs`, from the host's `--timings` on vite (2 runs, about 1.15 s in the
host). The `ts-quickjs` arm adds about 140 ms of Node on top: two Node
startups, the CLI, and hashing the bundle-cache inputs.

| Phase | ms |
|:---|---:|
| host startup + bundle load | ~3.5 |
| file selection + reads (portable walker, interpreted in QuickJS) | ~575 |
| parse (Rust, on the JS thread, inside dispatch) | ~338 |
| transforms | ~122 |
| result conversion | ~2 |
| commit | ~54 |

### Caveats

- **The `yaml` wall time is quantized by a polling sleep.** After it starts a
  workflow, `codemod workflow run` polls the run's status in
  `wait_for_workflow_completion` (`crates/cli/src/workflow_runner.rs`). It
  sleeps 1 s between checks until the run reaches `Running`
  (`workflow_runner.rs:239`), and 2 s while it is `Running`
  (`workflow_runner.rs:227`). On the two-file probe the arm uses 0.28 s of
  CPU but takes 1.34 s. On vite, the 12-thread transform work (about 2.7 s of
  user CPU) finishes inside the first sleep. So the `yaml` row reads as roughly
  "1 s + startup", not transform throughput. Its tie with `ts-quickjs` is a
  coincidence of that quantization. The sleep is CLI behaviour, not a harness
  artifact.
- **`ts-bridge` is faster than in phase A** (2.49 ± 0.28 s at `6d0ad87`, now
  1.93 ± 0.01 s). Three things changed:
  - the walker fast path for `relative()` in `execution/files.ts` (from the
    quickjs branch), which speeds up selection on every TS arm;
  - the CLI option-order fix, which lets the harness use
    `run --runtime=bridge <workflow>`;
  - a quieter, longer measurement: phase A used `--warmup 1 --runs 5`, with
    σ 0.28 s.

  Its probe floor also dropped, from 0.42 s to 0.31 s.
- **Parallelism differs by design per arm.** Each arm ran with its natural
  defaults:
  - `yaml`: 12 transform threads; user + system time is about 2.5× wall time.
  - `ts-bridge`: one bridge process per command, serial.
  - `ts-node`: one JS thread, plus napi parse workers; user + system time is
    about 2.1× wall time.
  - `ts-quickjs`: strictly one thread for JS and parsing; user + system time
    is about 1.1× wall time.
- **`ts-quickjs` is dominated by interpreted selection.** About 575 ms goes to
  the portable walker and file reads running in the QuickJS interpreter
  (`ts-node` does the same selection in about 176 ms on V8). Another ~340 ms
  is parsing on the JS thread. Transforms take about the same time in both
  runtimes (~120 ms). The fixed floor favours `ts-quickjs`: 0.15 s through
  Node, and 14 ms for the host alone.
- **The `ts-quickjs` bundle cache is warm** in the main table (warmup runs
  fill it). A cold cache (b) adds about 160 ms (esbuild and TypeScript load
  and build the bundle). The fully Node-free path (a) saves another
  ~140 ms.
- The run is too short to measure engine throughput for `yaml`. That would
  need a larger input, or a shorter poll interval in the CLI.

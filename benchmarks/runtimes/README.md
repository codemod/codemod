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
- `ts-node` / `ts-quickjs`: one JS thread runs the workflow and every
  transform, serially per file.

`ts-node` and `ts-quickjs` are skipped automatically, with a message, until
those runtimes exist. Every arm is probed first by running it on a two-file
scratch repository and checking the exact expected edit. `ts-bridge` switches
to `codemod-workflow run --runtime=bridge` by itself once the CLI accepts that
form. When the runtimes land, the harness needs no edits. If the CLI shape
ends up different, set `BENCH_CMD_TS_NODE=...` / `BENCH_CMD_TS_QUICKJS=...`
(see `lib.sh`).

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
cargo build --release -p codemod -p butterflow-execution-bridge
brew install hyperfine                          # or: cargo install hyperfine

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
`--prepare 'git -C "$REPO" reset -q --hard && git -C "$REPO" clean -fdq'`.
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

Machine: MacBook Pro `Mac16,7`, Apple M4 Pro, 14 cores (10 performance + 4
efficiency), 48 GB RAM, macOS 26.5.2 (25F84), arm64. node v24.18.0, codemod
1.13.18 (release build of this branch, base `6d0ad87`), hyperfine 1.20.0.
Recorded 2026-11-06, `--warmup 1 --runs 5`. Correctness gate: both arms gave
331 files and diff sha256 `8f7ff40f…c1c95885`.

| Arm | Mean [s] | Min [s] | Max [s] | User [s] | System [s] | Relative |
|:---|---:|---:|---:|---:|---:|---:|
| `yaml` | 1.416 ± 0.070 | 1.349 | 1.531 | 2.888 | 0.645 | 1.00 |
| `ts-bridge` | 2.488 ± 0.280 | 2.296 | 2.964 | 2.237 | 0.278 | 1.76 ± 0.22 |
| `ts-node` | not available yet | | | | | |
| `ts-quickjs` | not available yet | | | | | |

Fixed cost per arm, measured the same way on the two-file probe repository
(not part of `run.sh`):

| Arm | Mean [s] | User [s] | System [s] |
|:---|---:|---:|---:|
| `yaml` | 1.236 ± 0.015 | 0.221 | 0.045 |
| `ts-bridge` | 0.416 ± 0.118 | 0.483 | 0.051 |

How to read these numbers:

- **The `yaml` wall time is mostly a polling sleep, not work.** After it
  starts a workflow, `codemod workflow run` polls its status
  (`crates/cli/src/workflow_runner.rs`, `wait_for_workflow_completion`). It
  sleeps 1 s between checks while the run is not yet `Running`, and 2 s while
  it is. On a two-file repository the arm uses 0.27 s of CPU but takes 1.24 s.
  On vite the 12-thread transform work (about 2.9 s of CPU) finishes inside
  that first sleep. So the `yaml` number is quantized to that poll interval:
  it is roughly "1 s + startup", and the work it hides is under about 0.2 s of
  wall time. This is an engine/CLI behaviour, not a harness artifact. It
  affects any short `codemod workflow run`.
- **`ts-bridge` is serial.** About 0.4 s is fixed cost (Node startup, TypeScript
  stripping, the esbuild bundling of the two transforms, two bridge spawns).
  The remaining ~2.1 s is two bridge processes each transforming 1621 files on
  one thread, plus Node-side selection and writes. User time below wall time
  confirms there is no parallelism.
- A larger input, or a fix for the CLI poll interval, would be needed before
  the `yaml` row measures transform throughput. The `ts-*` rows measure it
  already.

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
  with a fresh QuickJS runtime per file. File selection (the TypeScript
  walker), reads, conflict checks, and writes happen in Node.
- `ts-node`: one Node process (the CLI strips TypeScript in-process through
  `bin/register-ts.mjs`; no re-spawn). One JS thread runs the workflow and
  every transform, serially per file. The napi addon walks and reads the
  selection natively (`crates/orchestration-walker`, worker threads), and
  plans, contains, parses, selector-matches, and converts results with the
  bridge's own batch rules (`crates/jssg-batch`), parsing 64-file chunks on
  worker threads one chunk ahead of the transforms.
- `ts-quickjs`: Node builds the bundle, or reuses it from its content-hash
  cache, then spawns `orchestration-quickjs-host`. One QuickJS context runs
  the control plane, the workflow, and every transform on one thread,
  serially per file. File selection and reads are native host primitives
  (the same `crates/orchestration-walker`, worker threads); batch rules are
  `crates/jssg-batch`, and Rust parses each file on the JS thread.

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

File selection is the same in both engines because every walker has the
engine's semantics (`packages/orchestration/src/execution/files.ts`, pinned by
`fixtures/walker/cases.json`; the native walker used by `ts-node` and
`ts-quickjs` is held to it by `packages/orchestration/tests/files-native.test.ts`).
Sources reach transforms verbatim in every arm (a leading BOM included, as
the engine's `read_to_string` does; see "Source encoding" in
`packages/orchestration/RUNTIMES.md`). The globs are written out explicitly in both
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
# lib.sh pins the release quickjs host and napi addon (CODEMOD_QUICKJS_HOST_BIN,
# CODEMOD_JSSG_NAPI) instead of letting the runtimes pick the newer debug build.

benchmarks/runtimes/setup.sh                    # clone to $HOME/.cache/codemod-bench/vite
benchmarks/runtimes/verify.sh                   # correctness gate only
benchmarks/runtimes/run.sh                      # quick path: gate + one hyperfine session
benchmarks/runtimes/preflight.sh                # machine checks only
benchmarks/runtimes/rounds.sh                   # gate + interleaved rounds + report (one size)
benchmarks/runtimes/sizes.sh                    # rounds at probe, vite, vite-x4, vite-x16 + scaling fit
benchmarks/runtimes/overhead.sh                 # cost of CODEMOD_TIMINGS_FILE per arm
node benchmarks/runtimes/compare.mjs old/report.json new/report.json
pnpm test:bench                                 # analysis tests on the fixtures
```

`run.sh` is the quick path. For numbers that will be compared or published,
use `rounds.sh` or `sizes.sh` (see "Rounds, sizes, and analysis" below).

Knobs (environment): `BENCH_REPO` (checkout path; `setup.sh` also takes it as
its argument), `BENCH_RUNS` (default 5), `BENCH_WARMUP` (default 1),
`BENCH_ARMS`, `BENCH_REQUIRE_ARMS` (default `yaml ts-bridge`, which fail
instead of being skipped), `CARGO_TARGET_DIR` / `CODEMOD_BIN` /
`CODEMOD_BRIDGE_BIN`, `BENCH_CMD_<ARM>`, `BENCH_TIMEOUT` (seconds; kills any
single arm run that exceeds it, so a hang fails instead of stalling the
session), and `BENCH_HOME`. `BENCH_HOME` runs
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

## Rounds, sizes, and analysis

`run.sh` runs each arm's runs back to back in one hyperfine session. Slow
drift (thermal state, background load, a cache warming up) then lands on
whichever arm happened to run at that moment. The round-based harness is
designed so that drift and noise show up in the numbers instead.

### Launching: `bench-exec`

Every arm starts as `bin/bench-exec [-t seconds] <arm command...>`, a small C
program (`bench-exec.c`) that `lib.sh` compiles with `cc` on first use into
the git-ignored `bin/`. It sets `BENCH_T0_NS` to the monotonic clock
(`clock_gettime_nsec_np(CLOCK_UPTIME_RAW)` on macOS) and `execvp`s the arm in
the same process. The timed span therefore has no extra process, and an
instrumented arm can report `exec_to_main_ns` (process start until `main`).
With `BENCH_TIMEOUT`, `-t` arms an `alarm` before the exec, so a hung run dies
with SIGALRM (exit 142). This replaces the old `perl -e 'alarm ...'` wrapper.
Arm command lines are plain argv (no shell syntax), so hyperfine can run them
with `-N` (no intermediate shell, no shell-spawn correction).

### `preflight.sh`

`preflight.sh` prints the machine spec block (model, CPU, RAM, OS, toolchain,
worktree sha + branch + dirty flag, bench repo commit), then one
PASS/WARN/FAIL line per check:

| Check | Fails when |
| --- | --- |
| power | on battery (`pmset -g batt`) |
| Low Power Mode | `pmset -g` reports `lowpowermode 1` / `powermode 1` |
| thermal | `pmset -g therm` reports `CPU_Speed_Limit` < 100 |
| load | 1-minute load average >= `BENCH_MAX_LOAD` (default 2) |
| busy processes | any process outside the harness's own ancestry is above `BENCH_MAX_PROC_CPU` % CPU (default 20); offenders are listed |
| Spotlight | `mds`/`mdworker` together above 5 % CPU (indexing) |
| worktree | the codemod worktree has uncommitted changes |
| binaries | a release binary for an arm in `BENCH_ARMS` is missing, or is a debug build |
| tools | `node`, `hyperfine`, `cc`, `git`, or `perl` missing |

It exits 1 on any failure. `BENCH_PREFLIGHT=warn` downgrades failures to
warnings, and `BENCH_PREFLIGHT=off` skips the checks (the spec block is still
printed). The output is saved with every run and embedded in the report.

### `rounds.sh`

1. Preflight (saved to `<out>/preflight.txt`), then the `verify.sh` gate. The
   gate's diff hash and file count are recorded in `results/arms.env`.
2. Warmup: one untimed run of every arm.
3. `BENCH_ROUNDS` rounds (default 10). Each round shuffles the arm order
   (seeded by `BENCH_SEED` and the round number, so it is reproducible) and
   runs each arm once:
   `hyperfine -N --runs 1 --prepare '<prepare.sh> <timings file>' --export-json raw/<round>-<arm>.json`,
   with `CODEMOD_TIMINGS_FILE=raw/<round>-<arm>.timings.json` set. The
   generated `prepare.sh` resets and cleans the repository, deletes stale
   timings files, and in cold mode also wipes the workflow cache.
4. After every timed run, the repository's diff hash must equal the gate's
   (`BENCH_CHECK_RUNS=0` turns this off). So every sample is a correct run.
5. `provenance.mjs` writes `meta.json`, and `analyze.mjs` writes the report.

Cache modes (`BENCH_CACHE`): `warm` (default) keeps the CLI's caches across
runs. `cold` points `CODEMOD_WORKFLOW_CACHE` at `<out>/workflow-cache` (unless
it is already set) and wipes it before every run. Add `BENCH_PURGE=1` to also
run `sudo -n purge` before every run, dropping the OS file cache; run
`sudo -v` first.

Output: `results/runs/<UTC stamp>/<size>/` (git-ignored) holding `raw/`,
`logs/`, `prepare.sh`, `arms.tsv`, `preflight.txt`, and `meta.json`.
`results/report.md` and `results/report.json` are also git-ignored. Copy
what you want to keep.

### `sizes.sh`

`sizes.sh` runs `rounds.sh` once per size in `BENCH_SIZES` (default
`probe vite vite-x4 vite-x16`) under one session directory and one shared
preflight, then analyzes them together. Every size is a git repository, so the
gate works unchanged:

| Size | Repository | Candidate files | Expected changes |
| --- | --- | ---: | ---: |
| `probe` | `make_probe_repo` (two files) | 2 | 1 |
| `vite` | the pinned checkout | 1615 | 331 |
| `vite-x4` | 4 copies in `copy-0/`..`copy-3/` | 6460 | 1324 |
| `vite-x16` | 16 copies | 25840 | 5296 |

For a `vite-xN` size, each copy is `git archive` of the pinned commit
extracted into its own subdirectory. The copies keep their own `.gitignore`
files, so selection and the expected diff are exactly N times vite's. The
result is committed once with a fixed author and date, which makes its commit
id (and so `BENCH_EXPECT_SHA`) reproducible. Any N works. The repositories
live in `BENCH_SIZES_ROOT` (default `<BENCH_REPO>-sizes`). They are rebuilt
only when missing or stale, and take about N times the checkout's disk
space. "Candidate files" is `git ls-files` matching the include globs outside
`node_modules`. It is the N used in the scaling fit. It runs slightly below
what the walkers select (1621 on vite, see "The pinned input"), but it scales
exactly with the number of copies, which is all the fit needs.

### `analyze.mjs`

`node analyze.mjs <size or session dir>... [--out dir] [--seed N] [--resamples N]`
uses Node only (no dependencies) and writes `report.md` and `report.json`:

- Per arm and size: n, median, IQR, IQR/median, min, max, median user and
  system time, and a percentile-bootstrap 95 % CI (10 000 resamples, seeded)
  of the ratio of the arm's median to the fastest arm's median. Each resample
  draws both arms' runs independently with replacement.
- Scaling: `wall = fixed + per_file x N` fitted by least squares to the
  per-size medians. The CIs on both terms come from a stratified bootstrap
  (each size's runs are resampled, then refitted), plus R^2 and a log-log
  ASCII plot of median wall time against N.
- Phases, from the timings files: each run's top-level file plus every child
  file are merged. Children come from the `children` list, falling back to
  `<top>.<pid>.json` beside the top-level file, and unlisted siblings are
  picked up too. Phases sum across processes. The report shows the median ms
  per phase, plus coverage (sum of phases / in-process wall), `exec_to_main`,
  in-process wall, time after the exit hook (hyperfine wall − exec_to_main −
  in-process wall), `idle_wait` and the poll lag (`poll_saw − engine_done`)
  for `yaml`, the process count, and how many runs had timings. Without
  timings files, the section says so.
- Noise check: the script exits 2 when any arm's IQR/median exceeds 5 %,
  unless `BENCH_ALLOW_NOISY=1`. The reports are written either way.
- Provenance: worktree sha + dirty flag, sha256 of `codemod`, the bridge, the
  quickjs host, the napi `.node`, and `bench-exec`; node/rustc/hyperfine/codemod
  versions; input tag and commit; cache mode; arm commands; and the preflight
  output.

`report.json` keeps every raw sample (wall and per-phase), so reports can be
compared later without the raw directories.

### `compare.mjs`

`node compare.mjs old/report.json new/report.json [--threshold 0.03]` prints,
for every size and arm in both reports and every phase both have, the change
of the median (new/old − 1) with a bootstrap 95 % CI. A change is flagged
`REGRESSION` when it is above the threshold (3 %) and its CI excludes 0, and
`improvement` likewise in the other direction. The script exits 1 on any
regression.

### `overhead.sh`

`overhead.sh` measures the cost of the timings instrumentation itself. After
the gate and a warmup, it runs every arm `BENCH_OVERHEAD_RUNS` times (default
10) with `CODEMOD_TIMINGS_FILE` unset and the same number of times with it
set. The off/on runs are interleaved in seeded random order, with the same
launcher and prepare step as `rounds.sh`. `overhead.mjs` then prints
median(on) / median(off) per arm with a bootstrap 95 % CI, a verdict against
the 1 % budget (`within`: the CI's upper end is <= 1.01; `OVER`: its lower
end is above it; otherwise `inconclusive`), and how many "on" runs produced a
timings file. It exits 1 when an arm is `OVER`.

### Timings files

Arms built with timings support write one JSON file per process when
`CODEMOD_TIMINGS_FILE=<path>` is set: `<path>` for the top-level process,
`<path>.<pid>.json` for children (listed in the top-level file's
`children`). Schema 1:
`{ schema, arm, pid, parent_pid, exec_to_main_ns, wall_ns, phases: { <phase>: { ns, count, cpu_ns? } }, marks, threads_used?, children }`.
The phases are `boot plan select read setup parse transform convert conflict
commit idle_wait`, and the marks are `engine_done` and `poll_saw` (yaml). An
example file per arm, with child files for the bridge and quickjs-host
processes, is in `fixtures/timings/`. Those files drive the analysis tests
(`tests/analysis.test.mjs`, `pnpm test:bench`).

## Results

All four arms were measured in one session (rt/integration after phase D:
`jssg-batch`, native selection, the single-process CLI, the verbatim-source
and quickjs cancellation fixes). No other builds or benchmarks ran; the
desktop session (browser, WindowServer) was active, load average 2.5 at the
start and 3.2 at the end.

- Machine: MacBook Pro `Mac16,7`, Apple M4 Pro, 14 cores (10 performance + 4
  efficiency), 48 GB RAM, macOS 26.5.2 (25F84), arm64.
- Toolchain: node v24.18.0, rustc 1.94.1, hyperfine 1.20.0.
- Build: codemod 1.13.18, release builds of `rt/integration` at `a3aa624` in
  one `CARGO_TARGET_DIR`: `codemod`, `butterflow-execution-bridge`,
  `orchestration-quickjs-host`, and the release `jssg-napi` addon.
- Input: a fresh vite v8.3.6 checkout (`BENCH_REPO=~/.cache/codemod-bench/vite-final2`).
- Recorded 2026-11-06 with `BENCH_WARMUP=2 BENCH_RUNS=10 BENCH_TIMEOUT=120 run.sh`
  (every run under a 120 s `alarm`; none came close).
- Correctness gate: all four arms gave 331 files and diff sha256
  `8f7ff40f81b3063b7594dc9d78ce1d290e1dda34bfc389edeb3f8d01c1c95885`, as did
  the direct host run in (a) below.

| Arm | Mean [s] | Min [s] | Max [s] | User [s] | System [s] | Relative |
|:---|---:|---:|---:|---:|---:|---:|
| `ts-node` | 0.368 ± 0.008 | 0.359 | 0.387 | 0.735 | 0.452 | 1.00 |
| `ts-quickjs` | 0.703 ± 0.016 | 0.693 | 0.748 | 0.603 | 0.348 | 1.91 ± 0.06 |
| `yaml` | 1.346 ± 0.006 | 1.334 | 1.351 | 2.630 | 0.658 | 3.66 ± 0.08 |
| `ts-bridge` | 1.772 ± 0.037 | 1.721 | 1.852 | 1.548 | 0.228 | 4.82 ± 0.15 |

Previous (phase C, `f8ae6be`, before phase D; same machine and settings):

| Arm | Mean [s] | Min [s] | Max [s] | User [s] | System [s] | Relative |
|:---|---:|---:|---:|---:|---:|---:|
| `ts-node` | 0.688 ± 0.003 | 0.686 | 0.692 | 1.147 | 0.304 | 1.00 |
| `ts-quickjs` | 1.320 ± 0.012 | 1.307 | 1.346 | 1.276 | 0.181 | 1.92 ± 0.02 |
| `yaml` | 1.324 ± 0.003 | 1.322 | 1.331 | 2.655 | 0.636 | 1.92 ± 0.01 |
| `ts-bridge` | 1.931 ± 0.011 | 1.915 | 1.944 | 1.868 | 0.238 | 2.81 ± 0.02 |

Extra measurements, from the same session and build, also with
`--warmup 2 --runs 10` (phase C values in the last column). They are not
part of `run.sh`:

| Measurement | Mean [s] | Min [s] | Max [s] | User [s] | System [s] | Phase C [s] |
|:---|---:|---:|---:|---:|---:|---:|
| (a) `orchestration-quickjs-host run <prebuilt bundle> --target <vite>` (no Node) | 0.666 ± 0.012 | 0.657 | 0.696 | 0.572 | 0.336 | 1.180 |
| (b) `ts-quickjs`, cold cache (`CODEMOD_WORKFLOW_CACHE` wiped before every run) | 0.901 ± 0.021 | 0.886 | 0.954 | 0.896 | 0.460 | 1.479 |
| (c) `ts-node`, cold cache (`CODEMOD_WORKFLOW_CACHE` wiped before every run) | 0.576 ± 0.012 | 0.560 | 0.593 | 1.018 | 0.548 | n/a |

The cold-cache rows wipe everything the CLI caches (type-stripped sources,
V8 compile cache, rewritten workflow modules and artifacts, quickjs bundles),
so TypeScript and esbuild load and run every time.

(d) Fixed floor per arm on the two-file probe repository (`make_probe_repo`
in `lib.sh`):

| Arm | Mean [s] | User [s] | System [s] | Phase C [s] |
|:---|---:|---:|---:|---:|
| `yaml` | 1.350 ± 0.007 | 0.232 | 0.057 | 1.341 |
| `ts-bridge` | 0.075 ± 0.004 | 0.051 | 0.021 | 0.309 |
| `ts-node` | 0.045 ± 0.001 | 0.041 | 0.012 | 0.275 |
| `ts-quickjs` | 0.045 ± 0.001 | 0.038 | 0.012 | 0.150 |
| quickjs host on a prebuilt bundle | 0.012 ± 0.000 | 0.009 | 0.004 | 0.014 |

### Where the time goes

`ts-node`, in-process timers from `profile-node.mjs` (3 runs on vite, which
agree within a few ms). About 335 ms of the 368 ms wall time is in the
process; the rest is Node boot before the first module, and exit:

| Phase | ms | Phase C |
|:---|---:|---:|
| import orchestration (cached type stripping) | ~13 | ~182 |
| load workflow + artifacts (cached; no esbuild) | ~2 | ~27 |
| file selection: walk (native, napi) | ~43 | ~176 (walk + reads, TypeScript) |
| file selection: reads (native, worker threads) | ~28 | (above) |
| parse (napi, worker threads; overlaps transforms) | ~206 (~65 blocks the JS thread) | ~210 (~80) |
| transforms (JS thread) | ~124 | ~120 |
| result conversion (`jssg-batch` through napi) | ~1-11 | (TypeScript) |
| stage + commit | ~26 | ~21 |

`ts-quickjs`, from the host's `--timings` (3 runs each, direct and through
the CLI, which agree within 10 ms). About 650 ms is in the host; the arm adds
~37 ms of Node (one Node startup, the CLI, hashing the bundle-cache inputs;
phase C: ~140 ms with two Node startups):

| Phase | ms | Phase C |
|:---|---:|---:|
| host startup + bundle load | ~3.7 | ~3.5 |
| file selection: walk (native, worker threads) | ~42 | ~575 (walk + reads, interpreted) |
| file selection: reads (native, worker threads) | ~30 | (above) |
| parse (Rust, on the JS thread, inside dispatch) | ~346 | ~338 |
| transforms | ~126 | ~122 |
| module load (both artifacts, once) | ~6.5 | |
| result conversion | ~2.4 | ~2 |
| commit | ~58 | ~54 |

### Caveats

- **The `yaml` wall time is quantized by a polling sleep.** After it starts a
  workflow, `codemod workflow run` polls the run's status in
  `wait_for_workflow_completion` (`crates/cli/src/workflow_runner.rs:112`). It
  sleeps 1 s between checks until the run reaches `Running`
  (`workflow_runner.rs:239`), and 2 s while it is `Running`
  (`workflow_runner.rs:227`). On the two-file probe the arm uses 0.29 s of
  CPU but takes 1.35 s. On vite, the 12-thread transform work (about 2.6 s of
  user CPU) finishes inside the first sleep. So the `yaml` row reads as roughly
  "1 s + startup", not transform throughput. The sleep is CLI behaviour, not a
  harness artifact. Measuring engine throughput for `yaml` needs a larger
  input or a shorter poll interval in the CLI.
- **What phase D changed.** `ts-node` halved (0.69 -> 0.37 s): one Node
  process with cached type stripping (~200 ms less startup) and native
  selection (~105 ms less). `ts-quickjs` almost halved (1.32 -> 0.70 s):
  native selection replaced ~575 ms of interpreted walking with ~72 ms, and
  the CLI lost its second Node startup. The fixed floors fell for every TS
  arm (one Node process, lazy TypeScript/esbuild, lazy executors).
  `ts-bridge` gained a little from the startup work (1.93 -> 1.77 s); it
  keeps the TypeScript walker and one bridge process per command.
- **`ts-quickjs` is now dominated by parsing on the JS thread.** ~346 ms of
  the host's ~650 ms is tree-sitter parsing that `ts-node` does on worker
  threads (where only ~65 ms blocks its JS thread). Transforms cost the same
  in both (~125 ms). Parsing off-thread in the host (roots are `Send + Sync`
  in `sg_core`; only the JS wrappers need the JS thread) is the obvious next
  step.
- **Parallelism differs by design per arm.** Each arm ran with its natural
  defaults:
  - `yaml`: 12 transform threads; user + system time is about 2.4× wall time.
  - `ts-bridge`: one bridge process per command, serial; about 1.0×.
  - `ts-node`: one JS thread, plus napi walk/read/parse workers; about 3.2×.
  - `ts-quickjs`: one thread for JS and parsing, plus walk/read workers;
    about 1.4×.
- **Caches are warm in the main table** (warmup runs fill them). Cold, (b)
  and (c), TypeScript and esbuild load and build: about +200 ms for both TS
  arms. The fully Node-free quickjs path (a) is ~37 ms faster than the arm.
- **System time is higher than in phase C** for `ts-node` and `ts-quickjs`
  (0.45 s and 0.35 s, from 0.30 s and 0.18 s): native walks and reads on
  worker threads trade single-thread user time for system time across
  several threads.

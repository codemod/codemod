# Handoff: workflow v2 engine perf (worker pool, memory, matcher work)

This branch sits on Mo's `engine/code-first` at `65362215` ("fix(workflow): remaining review fixes for the dependency APIs").

It holds five perf commits, one per optimization, so each can be reviewed or dropped on its own. It also holds the benchmark harness and the results, under `bench/engine-perf/`.

It is not a PR. Nothing here has been shown to Mo yet.

## Goal

Dev owes Mo an answer to three questions about the v2 engine, from a fair same-machine benchmark:

1. Is it faster without a worker pool?
2. Can a worker pool like the one in Dev's TS orchestration prototype be built on Mo's engine, and how far does it go?
3. Why does Mo's engine use more memory than v1?

Dev then widened the ask:

- Do a full CPU and memory profile, then build and measure every optimization, alone, in pairs and stacked.
- Explain each optimization's tradeoffs before deciding anything, keeping in mind that some changes only pay off together.

## Short answers

**1. Without a pool**

Mo's engine without a pool is about as fast as v1, at 0.9x to 1.1x of v1's time on most cells.

It is not faster than a pooled engine. The pool alone makes it 1.7x to 2.9x faster on every workload measured.

**2. Pool on Mo's engine**

Yes. The pool is commit `03dbdae5`, at about 370 lines.

On top of it, a per-thread rule cache and the kind skip make it 2.1x to 3.3x faster than Mo's head. That matches or beats Dev's prototype on every JS-transform cell.

Dev's prototype still wins clearly on the 14-step audit workflow (2.8x vs 1.8x on excalidraw). The likely reason is that Mo runs all 14 steps at once with 4 threads each, on 4 cores.

The fifth commit (cap) targets that case. Its numbers are still being collected.

**3. Memory**

Mo runs independent steps concurrently. Each step's `PendingFiles` keeps two full copies (`original` and `text`) of every file it reads, until the step ends.

- cal.com's JS/TS is 18.3 MB in 5,066 files, so that is about 36 MB per step.
- With 14 audit steps it reached 599 MB, against 56 MB for v1.
- heaptrack on a 2-step run: 72 of 81 MB at peak is `String::clone` in `PendingFiles::take`.
- `MALLOC_ARENA_MAX=1` barely changes it, so this is not allocator fragmentation.

Commit `90d1a891` drops a file's entry when the transform gives it back unchanged. It took audit on cal.com from 599 MB to 192 MB.

## The commits (oldest first)

| Commit | Change | Size | What it does |
|---|---|---|---|
| `90d1a891` | perf(sandbox): forget files a transform gives back unchanged | +24 / -2, `pending_files.rs` | When a transform returns a file unchanged, its entry leaves `PendingFiles` instead of holding two copies until the step ends. Test `a_file_given_back_unchanged_is_not_kept` added. |
| `03dbdae5` | perf(jssg): reuse one transform runtime per thread within a step | about +366 / -27, `execution_engine.rs`, `jssg_runner.rs` | Each walker thread keeps one QuickJS runtime per step (`PersistentTransform`); details below. |
| `b5e18f5e` | perf(jssg): let findAll skip node kinds a matcher can never match | +11, `ast_grep/utils.rs` and Cargo | `JsMatcherRjs` now forwards `potential_kinds`, so ast-grep's `find_all` skips nodes of kinds the rule can never match. It adds `bit-set = "0.11"`, which is already in the lockfile through ast-grep. |
| `46278f87` | perf(jssg): compile each pattern and rule once per thread | +45 / -5, `ast_grep/utils.rs`, `sg_node.rs` | A thread-local cache of compiled matchers; details below. |
| `c5a18d71` | perf(jssg): cap files transformed at once across concurrent steps | +38, `jssg_runner.rs` | A process-wide limit of one file in flight per core, taken before the file is taken from `PendingFiles`. |

More on the larger commits:

- **Pool (`03dbdae5`):**
  - The transform module is evaluated once, and `executeCodemod` is saved as a `Persistent<Function>`.
  - Each file then sets its per-file userdata, parses, and calls the saved function. GC runs every 64 files.
  - `CODEMOD_JSSG_REUSE_RUNTIME=0` turns it off. If the runtime can't load the transform, it falls back to the old per-file path.
- **Rule cache (`46278f87`):**
  - It is keyed by language plus the pattern string, or language plus the rule JSON. It is capped at 4,096 entries and cleared when full.
  - `convert_matcher` now returns `Rc<JsMatcherRjs>`.

The same changes also exist locally as separate branches: `perf/mem`, `perf/pool`, `perf/kinds`, `perf/rules`, `perf/cap`, plus the pair and stack branches. Everything in them is reachable from this branch by cherry-picking the commits above.

## How the numbers were made

**Machine**

- 4-core Intel Xeon @ 2.30GHz, 15 GB RAM, Linux, in a cloud container.
- Release builds, `cargo build --release -p codemod`.

**Harness** (`bench/engine-perf/harness/run.py`)

- Each cell is one workflow on one repo.
- Each cell gets one warmup round, then 3 measured rounds (2 on cal.com). Arms are shuffled every round.
- The repo is reset (`git reset --hard && git clean -fdx`) before and after every run.
- Recorded per run:
  - wall time;
  - children user+sys CPU;
  - peak RSS of the largest process (from `wait4`);
  - exit code;
  - sha256 of `git diff --cached --binary`;
  - files changed.
- Tables show medians.

**Repos** (pinned SHAs in `harness/setup.sh`): taxonomy, excalidraw, cal.com.

**Workflows**

| Workflow | Source | Edits files? |
|---|---|---|
| util-is | nodejs userland-migrations | Matches nothing in these repos, so it is a pure scan. |
| react-19 | react-codemod, 5 steps | Only cal.com changes (4 files). |
| debarrel | useful-codemods | 67 files on excalidraw. |
| audit | 14 detector steps, independent nodes | Read-only. |

The `ts` arm runs hand ports under `workflows/*/ts/workflow.ts`.

**Diffs:** every arm produced byte-identical diffs in every cell. Audit makes no edits, so its finding counts were not compared across arms (see "Not tested").

**`codemod:metrics`:** head 65362215 removed it. So every transform imports `workflows/_shared/metrics-shim.ts`, which counts in memory. v1 runs the same edited files.

**Arm names in the tables**

| Arm | Meaning |
|---|---|
| `mo` | head as is |
| `mem`, `pool` | single commits |
| `mem+kinds`, `mem+rules` | those two commits on top of mem. In `perf-main.csv` they are the arms `mo-kinds` and `mo-rules`. A branch mix-up put the memory fix under them, and pure single runs are in the second pass. |
| `all4` | mem + pool + kinds + rules |
| `ts` | Dev's prototype `ts-orch-quickjs-only` 682516f0, built from `/mnt/project-files/ts-orchestration-proof/patches/ts-orch/ts-orch-quickjs-only.bundle` |
| `v1` | npm CLI 1.18.7 |

## Results so far (perf-main.csv, medians, wall seconds, peak MB)

| Workload, repo | v1 | mo | mem | pool | mem+rules | all4 | ts |
|---|---|---|---|---|---|---|---|
| util-is, taxonomy | 2.02 | 0.35 | 0.36 | 0.16 | 0.33 | 0.13 | 0.11 |
| util-is, excalidraw | 4.02 | 2.13 | 2.04 | 1.11 | 1.86 | **0.85** | 1.04 |
| util-is, cal.com | 14.12 | 11.86 | 11.76 | 4.66 | 10.90 | **3.58** | 4.00 |
| react-19, taxonomy | 2.23 | 1.71 | 1.68 | 0.81 | 1.40 | **0.50** | 0.63 |
| react-19, excalidraw | 16.31 | 13.97 | 13.62 | 8.35 | 12.16 | **6.71** | 8.59 |
| react-19, cal.com | 80.35 | 72.47 (794 MB) | 71.94 | 35.89 (502 MB) | 61.40 | **26.03 (412 MB)** | 33.23 (726 MB) |
| debarrel, taxonomy | 2.01 | 0.68 | 0.64 | 0.23 | 0.61 | 0.21 | 0.19 |
| debarrel, excalidraw | 12.33 | 10.92 | 10.92 | 8.77 | 10.69 | **8.62** | 8.86 |
| debarrel, cal.com | 36.34 | 34.54 | 33.32 | 19.43 | 33.33 | **18.74** | 19.17 |
| audit, taxonomy | 2.01 | 1.86 | 1.79 | 0.74 | 1.97 | 0.64 | **0.30** |
| audit, excalidraw | 15.36 (73 MB) | 13.96 (437 MB) | 13.96 (275 MB) | 8.10 (446 MB) | 13.54 | 7.57 (277 MB) | **5.07 (137 MB)** |

`summary-main.txt` has every arm, including CPU seconds and peak MB for each cell.

Audit on cal.com was lost to a machine restart in the first pass. Earlier single runs of it, same machine:

| Arm | Wall | Peak memory |
|---|---|---|
| mo | 59 s | 599 MB |
| mem | 58 s | 192 MB |
| pool | 16.7 s | 540 MB |
| mem+pool | 17.3 s | 181 MB |
| v1, first 7 steps only | 43 s | 56 MB |

Mo's peak memory grows with the number of steps: 83, 126 and 330 MB for 1, 2 and 7 audit steps.

**What the numbers say**

- The pool is the big lever everywhere. It removes the per-file cost of creating a runtime and re-evaluating the transform, which dominates light transforms.
- The rule cache alone is worth about 1.1x to 1.2x on react-19.
  - With the pool it adds a lot more: react-19 on cal.com goes from 2.02x to 2.78x.
  - Once the pool removes startup, rule compilation is a much bigger share of what's left.
  - It also cut CPU from 125 to 84 s on that cell.
- The kind skip does almost nothing alone here. It reduces how many nodes a rule is tried on. Its effect alone vs. in pairs is in the pairs pass.
- The memory fix lowers peak memory wherever many steps or large repos meet (437 → 275 MB on audit, excalidraw) and costs no time.
- Dev's prototype beats all4 on audit. It runs the 14 detector steps with less oversubscription and at about half the memory.
  - This is the strongest argument for the cap commit, or for one shared worker pool per run instead of per step.
  - One rough run: all5 (all4 + cap) gave audit on excalidraw in 6.3 s and 14 s of CPU, against all4's 7.6 s and 20 s.

## CPU profile (perf, DWARF call graphs, react-19 on excalidraw, mem+pool build)

See `bench/engine-perf/profiles/perf-react19-excalidraw-inclusive.txt`.

| Where the time goes | Inclusive share |
|---|---|
| `findAll` | 55% |
| Compiling the rule on every call (`deserialize_rule`) | 16% |
| … of which regex builds | 11% |
| Trying the matcher on every node (no `potential_kinds`) | 30% |
| tree-sitter parsing | 29% |
| … the file itself | 12% |
| … the transform's own `parse()` calls | 12% |
| `Node::parent` (O(depth) in tree-sitter, used by inside/has rules) | 15% |
| malloc/free | about 10% |

The rule cache and kind skip came from this profile.

The memory profile is in `profiles/heaptrack-audit-2steps-peaks.txt`.

## Risks and costs, per change

**mem (`90d1a891`)**

- A later read of the same file within the step now goes back to disk. Before, it saw the step's in-memory snapshot.
  - Nothing in the step changed the file, so the only difference is a file edited on disk by something else mid-step.
- Low risk.

**pool (`03dbdae5`)**: the one with real behaviour changes.

- **Isolation:**
  - Module-level variables, globals and changes to built-in prototypes now live for the whole step on each thread, instead of for one file.
  - A transform that keeps a module-level cache, counter or array now sees data from earlier files on the same thread.
  - Which files share a thread depends on scheduling, so results that depend on that state are not deterministic across runs.
  - Shown by `bench/engine-perf/harness/bench/isolation`, run on 12 files:
    - `mo` stamps every file `seen=1`.
    - `pool` stamps `seen=1` to `seen=5` across 4 threads.
  - Dev's prototype evaluates the module once per worker, so it has the same property.
- **Errors:**
  - A transform that throws is handled per file, as before.
  - A transform that corrupts global state, or a QuickJS-level failure, leaves a runtime that is reused for the rest of the step. There is no "poison and rebuild" yet.
  - Runaway loops and cancellation inside a reused runtime were not tested.
- **Memory:**
  - Garbage accumulates between GC runs (every 64 files).
  - Each thread holds one runtime per step until the step ends, which matters when many steps run at once.
- **Complexity:**
  - About 370 lines of lifetime-sensitive rquickjs code. The `Persistent<Function>` must drop before the context and runtime, so field order matters.
  - A second code path to maintain beside the per-file one, which stays as the fallback.
- **Likely objection from Mo:** v2 is moving away from hidden state ("state needs to go away" in v2). Module scope that persists across files is a new hidden state channel, unless it is a documented contract.

**kinds (`b5e18f5e`)**

- No behaviour change: ast-grep guarantees that a matcher never matches a kind outside `potential_kinds`.
- `get_match_len` is still not forwarded. That is a separate question and was left alone.

**rules (`46278f87`)**

- No behaviour change, as long as compiled matchers hold no per-match state. ast-grep's are immutable after building.
- A cache key is built per call: one JSON stringify of the rule.
- Memory is bounded at 4,096 matchers per thread.
- **Wasm build:** the key uses the language's `Display`, and `WasmLang` has none, so the wasm build would not compile as is. It needs a small key function before review.

**cap (`c5a18d71`)**

- Threads that can't get a slot just wait, so it can't make total throughput much worse.
- It can stretch a short step while long steps hold the slots.
- Deadlock: the slot is taken before the file, so a transform waiting on another file never holds back the thread that has it.
- It does not reduce runtimes held per thread. Memory still needs fewer threads per step, or one shared pool.

## What was tested and what wasn't

**Tested**

- `cargo test --release -p codemod-sandbox` on mem+pool+kinds+rules: 136 passed, 0 failed.
- Identical diffs across all arms on 12 cells.
- The isolation demo.
- The `PendingFiles` unit test for the memory fix.

**Not tested**

- `butterflow-core`, CLI and workflow crate tests (`cargo test --workspace`).
- The wasm build.
- macOS and Windows.
- Audit finding counts across arms.
- Transforms that throw, hang or get cancelled inside a reused runtime.
- `jssgTransform` cross-file edits under the pool.
- Dry-run mode.
- Long runs for memory growth.
- The Solid migration assistant workflows. The v2 SMA needs the state-removal port first.
- The cap and pure single-change arms are in the second pass, still running when this was written.

## How to rerun

The harness has absolute paths under `/home/user/eb`. Copy it there, or replace that path throughout.

```bash
cp -r bench/engine-perf/harness /home/user/eb
cd /home/user/eb
bash setup.sh   # pinned repos and codemods, symlinks, metrics shim (needs network)
# Binaries go in /home/user/eb/bin: v1 from npm, Mo's head build at bin/mo/codemod,
# and one binary per commit combination (see harness/build4.sh for the pattern).
python3 run.py PERF results/perf-main.csv     # groups are defined in cells.py: PERF, AUDITCAL, SINGLES, PAIRS
python3 summarize.py results/perf-main.csv mo-kinds=mem+kinds mo-rules=mem+rules mo-stack4=all4
```

For one quick run:

```bash
./runwf.sh <binary-name> <workflow> <repo>
```

It prints wall, CPU, peak RSS, exit, diff hash and files changed.

Do not benchmark while a cargo build is running: 4 cores, and builds take about 4 minutes incremental, 16 cold.

## Decisions waiting on Dev (recommendations, not decisions)

**1. Whether to propose the pool to Mo, and with what isolation contract.**

Recommendation: propose it, but lead with the isolation change, not the speedup. The pool is where most of the gain comes from.

The suggested contract:

- "module scope runs once per worker thread per step";
- transforms must not rely on fresh module state;
- `CODEMOD_JSSG_REUSE_RUNTIME=0` as the escape hatch.

Expect pushback, since it is a state channel. The alternative, resetting globals between files, costs most of the gain.

**2. Which small changes to send first.**

Recommendation: send mem, kinds and rules as three small PRs before the pool. They are low risk and easy to review, and rules needs the wasm key fix first.

**3. Cap vs a single shared pool per run.**

Recommendation: wait for the second-pass numbers.

- If cap closes most of the audit gap with Dev's prototype, it is the cheap answer.
- If not, the next step is one worker pool per run shared across steps, as in Dev's prototype. That is a larger change to how steps are scheduled.

**4. How to present v1 vs v2.**

Mo's engine without these changes is roughly v1 speed, with much higher memory on multi-step workflows. With them it is 2x to 3x faster than v1 on most cells.

## Next steps

1. Finish the second pass (audit on cal.com, pure kinds/rules/cap, pairs) and add the numbers here. The local runner is `harness/perfrun2.sh`.
2. Fix the rules cache key for wasm. Add spec-style tests:
   - module state with the pool: a test that documents the contract;
   - a rule object reused across files gives the same matches.
3. Run `cargo test --workspace` on the stack.
4. Compare audit finding counts across arms. Have the shim print totals, or turn the detectors into marking transforms.
5. Further candidates found but not built:
   - parse once per file across concurrent read-only steps;
   - avoid the second walk (`count_files`) before each step;
   - store `PendingFiles` copies as shared `Arc<str>`;
   - an upstream ast-grep fix for O(depth) `parent()` in relational rules;
   - fewer threads per step when many steps run at once.
6. Run the SMA workflows once the v2 SMA port is on head.

Raw session notes and the full CSVs also live in the project files under `v2-worker-pool/` and `handoff/`.

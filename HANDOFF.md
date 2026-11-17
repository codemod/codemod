# Handoff: workflow v2 without state or metrics

Branch `handoff/workflow-v2-state-metrics-removal`, written 2026-11-17. It is PR #2491's head (`b1feab9`) plus one local commit of review fixes (`2d628ff`) and this file. No PR is open from this branch; the fixes are meant to go onto #2491.

## Goal

In workflow v2 (Mo's code-first engine, `engine/code-first`, from #2485, not on `main`), **workflow state and metrics are gone completely**. Dev's rule: "state needs to go away completely"; "migration assistant should not be using state or metrics".

Values move between steps as return values instead:

| Step | Returns | Replaces |
| --- | --- | --- |
| `jssg(path)` | `{ changes, unmodified, failures, data }`, where `data` is `[{ file, data }]` sorted by path | `useMetricAtom`, `getState`/`setState` |
| `jssg(path, { reduce, initial })` | same, but `data` is one folded value | metric totals |
| `shell(cmd)` | `{ stdout, stderr, exitCode }`, last 1 MiB of each stream | `CODEMOD_OUTPUT`, `result.values`, `result.output`, `$STATE_OUTPUTS` |

A transform returns `{ edit, data }`, and `data` is any JSON. A file that returns `data: null`, or no data, is left out of the list (Dev decided this, Nov 17).

```ts
// workflow.ts
const found = await step("Find", jssg("scripts/find.ts"));
// found.data: [{ file: "src/a.tsx", data: { buttons: 2 } }, ...]

const total = await step("Count", jssg("scripts/find.ts", {
  initial: 0,
  reduce: (sum, { data }) => sum + (data as { buttons: number }).buttons,
}));
// total.data: 7

const { stdout = "{}" } = await step("Detect", shell`node -p "JSON.stringify({ major: 19 })"`);
const { major } = JSON.parse(stdout.trim().split("\n").at(-1)!);
```

## How this relates to the PRs

| PR | State | What it holds |
| --- | --- | --- |
| #2489 | Merged into `engine/code-first` (squash `4b31a1c`) | Removes workflow state; transforms return `{ edit, data }`; removes `codemod/metrics` |
| #2486, #2488 | Merged by Mo | Dependency APIs; relative workflow imports. Mo followed with his own fix commits (`67ac941`, `0f65c62`, `6536221`) |
| #2491 | Open, base `engine/code-first`, head `claude/project-thread-qkxezu` at `b1feab9` | Everything below. No reviewer requested; waits on Mo |
| #2490 | Open | v2 tutorial; still uses `CODEMOD_OUTPUT` and metrics, needs the same changes (see Next steps) |
| #2492-#2494 | Open (Dev's) | Lockfiles and `codemod/deps`; separate work, not touched here |

Commits on #2491, oldest first, all authored as Dev:

1. `9267b4f` jssg data is any JSON, listed per file or folded by a reducer
2. `32df5cf` shell steps return stdout, stderr and exitCode; `CODEMOD_OUTPUT` removed; YAML `$STATE_OUTPUTS` translated
3. `2a3b7d0` core README: the v2 engine has no workflow state
4. `0379cf0` keep the end of a single output line longer than 1 MiB
5. `380774e` review fixes: Node reducer `initial` copied per attempt, cut state lines fail instead of vanishing, reducer that catches `failStep` still fails
6. `95c10cf` second review round: reducers can use `semver`/registry, `TransformData` is JSON, heredoc delimiter can't collide, `data.json` names files by path
7. `b1feab9` a Node reducer that ran out of time stops folding

Only on this branch: `2d628ff` independent review fixes (next section).

Note: #2491 undoes Mo's env-var approach for translated state from `67ac941` (an engine-set `STATE_OUTPUTS` file). Mo may push back; that ties into the Windows decision below.

## What `2d628ff` fixes

Each item has a spec that failed before the fix, unless marked otherwise.

- **Non-UTF-8 output.** One invalid byte used to end stdout early with exit code 0, losing the JSON on the last line. Now read with `read_until` + `from_utf8_lossy`; bad bytes become U+FFFD. Spec: `crates/runners/tests/direct_runner.rs` `a_line_that_is_not_utf8_keeps_the_rest_of_the_output`.
- **`#! /usr/bin/env python3` (space after `#!`).** The runner checked `#!/`, the compat and shell tasks checked `#!`, so this ran through `sh -c`. Now `#!` everywhere. Spec: `a_script_whose_interpreter_follows_a_space_runs_with_that_interpreter`.
- **Old `metrics.json` fixtures.** `codemod jssg test` silently ignored them and wrote a fresh `data.json` that passes. Now the case fails with "delete metrics.json and run with --update-snapshots". Unit spec `an_old_metrics_json_fails_the_case` (`crates/cli/src/commands/jssg/test.rs`; not run red first). No `metrics.json` fixtures exist in this repo.
- **Docs and d.ts examples.** `ShellTask` is `Skippable<ShellResult>`, so `stdout` is `string | undefined` (a dry run skips shell steps). Examples in `workflow.d.ts` and `docs/community/workflows/reference.mdx` now default it; `docs/community/jssg/metrics.mdx` casts `data`. Both reference-page snippets were compiled under the strict `type-tests` config (not added as permanent type tests).
- **Fold order.** On the CLI the reducer gets files in completion order; on Node, sorted, after the transform ends. Docs now say a reducer's result must not depend on order, and that the Node host holds every file's data. Docs only: Node order was not shuffled.
- **Changeset** `.changeset/workflow-ts-types.md` lists the `data`, metrics and shell result changes.

## What was tested

Run on `2d628ff` locally (Linux), all green:

| Suite | Result |
| --- | --- |
| `cargo test -p codemod` (CLI) | 425 + 13 + 2 + 8 pass |
| `cargo test -p codemod-workflow` | all pass (unit, tasks, compat, engine suites) |
| `cargo test -p butterflow-runners` | 12 unit + 8 integration pass |
| `crates/cli/npm`: `npm test` | 55 pass, 0 fail |
| `crates/cli/npm`: `npm run -s typecheck` | pass |
| `cargo clippy -p butterflow-runners -p codemod --tests`, `cargo fmt --check`, oxfmt, oxlint (commit hook) | clean (19 oxlint warnings existed before) |

Not tested:

- **Windows.** Nothing ran on Windows. Translated state steps are known broken there (below).
- `codemod-sandbox`, `butterflow-core` and `codemod-node` suites were not rerun for `2d628ff` (they passed for `b1feab9`'s parents).
- No real-world package run end to end on v2 after these fixes (the Solid migration assistant ran on earlier commits).
- CI: only `license/cla` and Proofline run on PRs against non-`main` branches; the full CI matrix has not run on any of this.
- Insights (codemod-app) against v2: not run; see open issue 2.

## How to run and verify

```bash
git checkout handoff/workflow-v2-state-metrics-removal
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
cargo test -p butterflow-runners
cargo test -p codemod-workflow
cargo test -p codemod
cd crates/cli/npm && npm test && npm run -s typecheck
```

Single specs worth reading first, as executable docs of the rules:

- `crates/workflow/tests/tasks.rs`: jssg data list, reducer, `a_jssg_reducer_can_use_semver`
- `crates/workflow/tests/compat.rs`: translated `$STATE_OUTPUTS`, `a_script_with_its_own_interpreter_may_contain_any_line`
- `crates/runners/tests/direct_runner.rs`: output limits, non-UTF-8, `#!`
- `crates/cli/npm/tests/testing.test.mjs`: the Node host's reducer rules

## Open issues

1. **High: translated YAML steps that write state fail on Windows.** `crates/workflow/src/compat/mod.rs` `with_state_outputs` wraps any translated `run:` step that writes `$STATE_OUTPUTS` in POSIX shell (mktemp, subshell, heredoc). On Windows, `DirectRunner` sends non-`#!` commands to cmd.exe, where none of that works. Before #2491 the engine set `STATE_OUTPUTS` in the environment and left the command alone. Decision below.
2. **Medium: Insights depends on the metrics API v2 removes.** codemod-app `crates/grep-jssg/src/lib.rs` uses `MetricsContext`, `metrics_context` and `metrics.get_all()` from `codemod-sandbox`, pinned at `rev = "0d66d565..."`. Nothing breaks today. Bumping that pin past v2 breaks the build, and Insights would have to read `data` instead. `apps/worker/src/jobs/butterflow-workflow.ts` also mentions metrics (not read in detail). Stale docs here: `docs/community/package-structure.mdx` lines ~67 and ~285 (Insights discovers "data names"), `docs/enterprise/insights/jssg-queries.mdx` lines ~21-27 (metric cardinality keys). Related Linear: CDMD-5153.
3. **Low, not fixed:**
   - `jssg.rs` sorts `data` by string bytes; `outcome.matched` sorts as `PathBuf`. `a-b.ts` and `a/b.ts` order differently. `matched` is not in the step result, so nothing user-visible differs today.
   - After the CLI reducer fails, the transform still runs over every remaining file before the step fails (`jssg.rs` ~76-87). Fix: set the cancel flag when the fold ends in error.
   - File changes a reducer makes are dropped silently (`reduce.rs`, only `failure` is read).
   - Translated steps print the state marker and every state line into the step log; values may be sensitive.
   - `direct_runner.rs` `collect` buffers a whole line before the 1 MiB cut, so one huge line with no newline is held in memory.
   - `codemod jssg test` writes a missing `data.json` snapshot even under `CI`, so a new case passes its first CI run.
   - No way to type a transform's data on the step (`jssg<Data>(...)` would give `JssgData<Data>`).
4. **Tests the independent review asked for, not added:** non-JSON `data` (function, BigInt) failing a file through a workflow step; `forEachFile(selector, jssg(path, { reduce, initial }))` keeping the reducer; the same reducer on both hosts over several files giving equal results; `data: null` file absent by name (`src/c.ts` fixture); step timeout during the Rust reducer; a Windows run of a translated state step.
5. **Proofline bot on #2491:** two threads about `data: null` stay open by Dev's decision (left out on purpose). One re-check on the resolved state-cut thread said "still present" with no detail; reading `applyStateOutputs` at `b1feab9`, a missing marker throws and no silent-loss path was found, so it is likely a false positive.

The full independent review: `/mnt/project-files/review-2026-11-17/pr-2491.md` (project files, not in the repo).

## Decisions waiting on Dev

1. **Push `2d628ff` to #2491?** Recommendation: yes. It fast-forwards the PR branch (`git push origin 2d628ff:claude/project-thread-qkxezu`), no force needed.
2. **Windows for translated state steps.** Options:
   - (a) For translated steps only, the engine keeps a hidden temp file in an internal env var and reads it back after the step, as Mo's `67ac941` did. Works on every shell. The workflow.ts API stays stateless; this is compat plumbing only.
   - (b) Emit a cmd.exe variant of the wrapper on Windows. Two wrappers to keep in step; cmd quoting is fragile.
   - (c) `codemod workflow upgrade` flags such steps as POSIX-only and the run fails clearly on Windows. Cheapest, but Windows users of old YAML packages lose those steps.

   Recommendation: (a). It is the only option that keeps old packages working everywhere, it is invisible to v2 authors, and it matches what Mo already wrote, which also lowers the chance of a review fight on #2491. It does reintroduce an internal file, which sits uneasily with "state gone completely"; the case for it is that no workflow.ts author can see or use it.
3. **How Insights reads v2 results.** Recommendation: before anyone bumps codemod-app's `codemod-sandbox` pin, agree with Alex and Mo on a `data` shape Insights reads (for example, a transform returns rows that the dashboard aggregates), then update `grep-jssg` and the three doc passages together.

## Next steps

1. Get Dev's yes on decision 1, then push `2d628ff` to #2491.
2. Implement the Windows choice with a spec (a Windows CI job, or at least a unit test of the generated command).
3. Add the missing tests from open issue 4 that guard real rules (keep the suite lean: one rule per spec).
4. Update #2490 (tutorial): section 2 (data rows, cardinality, count, metrics printout), section 3 and the final listings (`CODEMOD_OUTPUT`/`files@=` to printed paths, split stdout), recap lines ~444/446.
5. Update the v2 blog draft's data and shell examples to match.
6. Docs follow-up in the `codemod` repo's Mintlify `docs/`: workflow reference, jssg metrics page, Insights pages, once the Insights decision is made.
7. Wait for Mo's review of #2491. Don't request reviewers or comment on GitHub without Dev.

## Conventions for whoever continues

- Commits are authored as `Dev Agrawal <devagrawal09@gmail.com>`, with no AI co-author trailer and no AI mention in PR text (keeps the CLA check clean).
- Ask Dev before pushing to a PR branch, opening PRs, commenting on GitHub or updating Linear.
- Tests: plain specs, one rule per test, fewer lines rather than more.

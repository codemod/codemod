# What three real workflows and a dashboard needed

Built on `prototype/typescript-orchestration` (#2411) as it stood on Nov 11:
three workflows in `workflows/`, run on real repos, and a local dashboard in
`dashboard/`. This is what the engine lacked, ranked by how much each one
unblocked. "Hacked" means the dashboard or a workflow works around it today.

| # | Primitive | Status | What happened |
|---|---|---|---|
| 1 | Event stream with timestamps | **Added** (`--events`) | The engine already had `RunEvent`s, but `runBundled` dropped them in a `CollectingSink`, they had no times, and the history is written only when the run ends. Nothing could show a step running, how long it queued or what it printed. `--events <file>` now appends one JSON line per event with `at`, plus a new `command.output` event: shell output and transform `console` output, attributed to the command through the async context. This unblocked the whole steps view. |
| 2 | Resume that retries | Hacked | A resumed run replays every recorded completion, `cancelled` included, so resuming a cancelled run fails again in 80 ms. Dropping only the completion is worse: a scheduled command with no completion replays as `unknown`. The dashboard drops both records of every `cancelled` or `failed` command before resuming, which works (the cancelled audit resumed, replayed 2 steps and ran 7). The engine should own this: `--retry cancelled,failed`, with `unknown` always stopping for a person. |
| 3 | Run store | Hacked | A run has no id, no record of its workflow, target, input, start or end, and its error exists only as the last line of stderr (with ANSI colour codes even when stderr is not a terminal). The dashboard keeps `.runs/<workflow>/<id>/{run.json,events.jsonl,history.json,stderr.log,output.json}` itself. The CLI should write this (`codemod workflow runs`), so any UI is a reader. |
| 4 | Workflow metadata and result shape | Hacked | A workflow can't say its title, what it does, its default target or an example input, or what its output looks like. Each workflow has a `lab.json` for the first four and a `view.js` for the results view, keyed by folder. An exported `meta` plus a result schema would let a dashboard pick a view and validate the output. |
| 5 | Metrics as step output | Hacked | The audit codemods count findings with `codemod:metrics`. That module loads in a v2 transform, but nothing collects the counts, so the step returns nothing. The nine detectors were regenerated to push findings into the transform's `output` instead. The jssg completion should carry the step's metrics. |
| 6 | Reusing a codemod | Missing | A `jssg()` step needs its transform written inline, and the build finds each `jssg()` call statically, so steps can't be made in a loop or from an existing package. There is no way to run a registry codemod (like `react/19/replace-act-import`) or a local `scripts/codemod.ts` as a step. The act migration rewrote that codemod inline; the audit wrapped nine detectors in one step. |
| 7 | `cwd` for shell | Hacked | `jssg` takes `target.root` per invocation; `shell` always runs in `--target`. Every per-repo shell command in the fan-out starts with `cd '<repo>' &&`. |
| 8 | Scheduling by cost | Missing | In the audit, four 20 ms `git` commands queued for 39 s behind the jssg batches: admission is FIFO by weight (jssg 2, capacity 4). Scheduling itself also stalls while transforms hold the one JS thread (a command issued in the same tick was scheduled 2 s late). Cheap steps need their own lane or a priority. |
| 9 | Cancel inside a jssg step | Missing | SIGTERM marked queued steps cancelled at once, but the jssg batch in flight ran another ~23 s before it stopped. Nothing was written, which is right, but cancel should land between files. |
| 10 | A Node-free build | Missing | The run path has no Node, but `cargo build -p codemod` fails until `pnpm install` has put `ignore` in `packages/orchestration/node_modules` (the build script embeds it). |

What worked as is: fan-out with `Promise.all` and per-invocation ids, `target.root` per repo, a `json` jssg step for `package.json`, typed shell output with `guard`, the same jssg step twice under two ids (before/after counts), replay (a finished run replays in about 100 ms), and cancellation that writes nothing.

## Checked against earlier results

- The audit on the same four commits matches the Nov 9 audit (CLI 1.18.6) once the five rules this workflow leaves out are removed: excalidraw 48, papermark 280, umami 25. formbricks was cloned at a newer commit (164).
- React 19 readiness ran on a sparse checkout of Backstage (`packages/` and eight plugins): 6 of 35 packages use a React 19 breaking API (3 `react-dom/test-utils` imports, 3 legacy `render` imports, 1 `propTypes`). It covers less of the repo than the Backstage case study, so the counts aren't comparable.
- The act migration rewrote those three imports, counted 0 after, and committed on a branch.

## Not tried

`agent()` and `assessment()` steps (no keys here), multi-repo writes, and the release build (all timings are from a debug build).

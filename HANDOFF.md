# Handoff: workflow-lab (workflow v2 workflows and a local run dashboard)

Branch `claude/project-thread-xa4azn` in codemod/codemod. No PR. Written Nov 17, 2026, for
someone picking this up cold. Everything for this work is in `workflow-lab/` plus a small
engine change listed below.

## In one paragraph

On Nov 11 Dev asked to take recent real work (the jssg React audit, the Backstage React 19
case study, the registry's `react/19/replace-act-import` codemod), write it as workflows on the
workflow v2 engine as it existed then, build a simple local dashboard over their runs, and let
the dashboard show which engine primitives are missing. All three workflows ran on real
repos, the dashboard works, and `workflow-lab/PRIMITIVES.md` ranks ten missing primitives.
**Since then the engine this was built on was overtaken by Mo's engine (#2485), which already
has most of those primitives.** The ideas carry over; the code needs porting before it is
useful again. See "What is stale".

## Goal (Dev's words, Nov 11)

> "take some of the recent work we've done or work in the codemod blog or registry, build
> workflows in workflow v2 thats implemented so far, and build some simple local dashbaord for
> them. lets start buildign dashbaords and then figure out what primitives are gonna be needed."

Dev's longer-term aim for the dashboard (from earlier conversations): two jobs, full
observability and control over the workflows in a folder, and showing custom UI that the user
or their agent builds over run results.

## What the branch is built on

- Base: `prototype/typescript-orchestration` at 3c2d7ea4 (Dev's TS engine prototype, PR #2411).
  That branch has one newer commit since (b71f40bc, Nov 11); this branch was not rebased onto it.
- Engine there: `@codemod.com/orchestration` (`packages/orchestration`, `crates/orchestration`).
  Workflows import from `@codemod.com/orchestration`, run with
  `codemod workflow run -w workflow.ts [--target] [--input] [--history]` in a QuickJS host in the
  Rust CLI (no Node at run time). Step kinds: `shell`, `jssg`, `agent`, `assessment`;
  composition with `dynamic`, `sequence`, `parallel`; history replay by command id.

## What's built

### 1. Engine change: `--events <file>` (the only engine change)

The engine already produced run events (`command.scheduled`, `command.completed`,
`run.finished`, ...) but threw them away, they had no timestamps, and history was written only
when a run ended, so nothing could show a step in progress.

- `codemod workflow run --events <file>` appends one JSON line per event, each with `at`
  (milliseconds), while the run is going.
- New event `command.output` carries a step's printed output (shell output and `console` output
  from a transform), tagged with the step's id.
- Files: `packages/orchestration/src/core/events.ts` (`JsonLinesSink`, the new event),
  `packages/orchestration/src/host/quickjs/entry.ts` (wires it up; uses the host's async context
  to know which step printed a line), and on the Rust side three small pass-throughs:
  `crates/cli/src/commands/workflow/run.rs` (the flag), `typescript.rs` and
  `crates/orchestration/src/host/{mod.rs,native.rs}` (pass the path down and add an
  `appendText` function the JS can call to append to a file). About 130 lines in all.
- Tests: `packages/orchestration/tests/events.test.ts` (one spec) and one CLI end-to-end spec in
  `crates/cli/tests/typescript_workflow.rs`
  (`the_event_stream_records_a_shell_steps_output_under_its_command`).

### 2. Three workflows (`workflow-lab/workflows/`)

| Workflow | Shape | What it does |
|---|---|---|
| `react-audit` | Fan-out over repos | Lists repos in a folder, then for each repo runs `git` info and one jssg step with nine React/Next.js detectors in parallel. Output: findings per repo and rule. |
| `react-19-readiness` | Report | A `json` jssg step over `package.json` files (declared React versions) and a `tsx` jssg step for React 19 breaking APIs, grouped per package. |
| `react-19-act-migration` | Transform and verify | Branch, count `react-dom/test-utils` `act` imports, rewrite them to `react`, count again (fails if any are left), `git diff --check`, commit. |

Each folder has `workflow.ts`, `lab.json` (title, description, default target and input, read by
the dashboard) and `view.js` (that workflow's results view).

- The nine audit detectors in `react-audit/rules/` were generated from the jssg audit
  (`/mnt/project-files/jssg-audit/src`), with metrics swapped for pushing findings into the
  step's output. **The audit's security rules were left out on purpose: keep third-party
  security findings out of anything shareable.**
- `react-19-act-migration` re-implements the registry codemod inline (see primitive 6).

### 3. Dashboard (`workflow-lab/dashboard/`)

- `server.mjs`: one-file Node `http` server, no dependencies, on `127.0.0.1:4319`. It starts
  `codemod workflow run` as a child process with `--history` and `--events`, and stores each run
  under `workflow-lab/.runs/<workflow>/<run-id>/` (`run.json`, `events.jsonl`, `history.json`,
  `stderr.log`, `output.json`). It folds the event lines into per-step status and timing.
- `public/`: plain HTML/CSS/JS, no build.
- What it shows and does:
  - per-workflow run list;
  - for each run: wall time, step count and replayed count, and per step its status, queue time,
    run time, a timeline bar, and output on click;
  - Start, Cancel (SIGTERM), "Resume from history" for failed or cancelled runs, and Replay for
    finished ones;
  - results tabs: the workflow's own view, raw JSON, and the engine's stderr.
- Why Node for the server: it only serves the page and starts `codemod` processes; the
  workflows themselves run in the codemod binary with no Node. The plan was for the CLI to own
  the run store so the page could be served by the CLI later.
- Screenshots: `workflow-lab/screenshots/` (also in `/mnt/project-files/workflow-lab/screenshots/`).

### 4. `workflow-lab/PRIMITIVES.md`

The full ranked list with evidence. Short version, ranked by how much each unblocked, with
status as of Nov 11 on #2411's engine:

1. **Event stream with timestamps:** added here (`--events`).
2. **Resume that retries:** resuming a cancelled run replayed the cancellation and failed in
   80 ms. The dashboard works around it by removing cancelled and failed steps from the history
   before resuming.
3. **Run store:** a run had no id or record (workflow, target, input, start, end, error). The
   dashboard keeps its own.
4. **Workflow metadata and result shape:** no title, description, default target, example input
   or output schema. Worked around with `lab.json` and `view.js`.
5. **Metrics as step output:** `codemod:metrics` counts went nowhere; detectors were rewritten
   to return findings instead.
6. **Reusing a codemod:** no way to run a registry or local codemod package as a step;
   transforms had to be written inline.
7. **`cwd` for shell:** every per-repo shell command starts with `cd '<repo>' &&`.
8. **Scheduling by cost:** four 20 ms `git` steps queued 39 s behind jssg batches (FIFO by
   weight).
9. **Cancel inside a jssg step:** a running jssg batch took about 23 s to stop after cancel.
10. **Node-free build:** `cargo build -p codemod` needs `pnpm install` first for one embedded npm
    package.

## What is stale now that #2485, #2489 and #2491 exist

- **#2485** (Mo's `engine/code-first`) is a separate v2 engine and is where v2 is going.
  Workflows there import from `codemod/workflow` (`step`, `shell`, `jssg`, `forEachFile`,
  `codemod`, `dependencyGraph`, `agent`, `approval`, `sleep`, ...). **Nothing in
  `workflow-lab/workflows/` runs on it as is**, and the `--events` change and the dashboard's
  server target #2411's CLI flags.
- **#2489** (merged into `engine/code-first` as 4b31a1c) removed workflow state and
  `codemod/metrics`; transforms return `{ edit, data }`.
- **#2491** (open, branch `claude/project-thread-qkxezu`, base `engine/code-first`): `jssg`
  data is any JSON, listed per file or folded by a reducer, and shell steps return `stdout`,
  `stderr` and `exitCode`.

Checked against `engine/code-first` and #2491's head on Nov 17 by reading the code (not by
running it):

| # | Primitive | On #2485 / #2489 / #2491 |
|---|---|---|
| 1 | Event stream | **Exists.** `--format jsonl` prints every engine event (run/step started, replayed, progress, log, retrying, finished) with `seq` and `ts`. It goes to stdout, not a file. |
| 2 | Resume that retries | **Exists.** `codemod workflow resume`; a step canceled mid-run "runs again on resume". |
| 3 | Run store | **Exists.** `crates/state/src/run_store.rs`: run record with status, start/end times, the workflow's return value and settings; one step record saved as each step finishes; stored under `<data dir>/codemod/runs`. CLI has `workflow list`, `status`, `cancel`, `resume`. |
| 4 | Workflow metadata and result shape | Not checked; probably still missing. |
| 5 | Metrics as step output | **Done differently** by #2489 and #2491: transforms return `data`, and the step result carries it. |
| 6 | Reusing a codemod | **Exists.** `codemod(source, { params })` runs a registry or local package as a step, with its own resumable history. `jssg()` also takes a file path. |
| 7 | `cwd` for shell | Not needed: the `shell` tagged template quotes values, so `` shell`cd ${dir} && ...` `` is safe. |
| 8 | Scheduling by cost | Not checked. |
| 9 | Cancel inside a jssg step | Not checked (there is a cancel flag in `crates/workflow/src/hosted.rs`). |
| 10 | Node-free build | Not checked. |

So most of the dashboard's workarounds (its own run store, the resume hack, the events file)
would become thin readers of what #2485's CLI already writes.

## What was tested and what wasn't

Tested (Nov 11, on this branch):

- `packages/orchestration`: 559 passed, 8 skipped; typecheck clean; `oxfmt --check` clean.
- `cargo test` CLI end-to-end `typescript_workflow`: 40 passed, including the new events spec.
- All three workflows ran on real repos through the dashboard:
  - **React audit** on umami, excalidraw, papermark, formbricks: 517 findings. Same counts as the
    Nov 9 audit (CLI 1.18.6) on the same commits once the left-out rules are excluded:
    excalidraw 48, papermark 280, umami 25. formbricks was at a newer commit: 164.
  - **React 19 readiness** on a sparse checkout of Backstage (`packages/` and eight plugins):
    6 of 35 packages use a breaking API (3 `react-dom/test-utils` imports, 3 legacy `render`
    imports, 1 `propTypes`). Not comparable to the Backstage case study, which covered more of
    the repo.
  - **act migration**: 3 imports before, 0 after, 3 files changed, committed on a branch in the
    lab clone.
  - Cancel and resume: a cancelled audit run resumed, replayed 2 steps and ran 7.
  - A finished run replays in about 100 ms. The audit takes about 44 s over four repos.

Not tested:

- `agent()` and `assessment()` steps (no API keys in that environment).
- Writing to several repos in one run.
- A release build (all timings are from a debug build).
- Anything on #2485's engine. The table above comes from reading the code on Nov 17.
- The dashboard on Windows or macOS (Linux only), and with more than a few dozen runs.

## How to run the dashboard

From the repo root on this branch:

```sh
pnpm install --filter @codemod.com/orchestration...   # the CLI build embeds one npm package
cargo build -p codemod                                # debug build; the server uses target/debug/codemod
workflow-lab/setup-repos.sh                           # shallow clones into ~/lab-repos
node workflow-lab/dashboard/server.mjs                # http://127.0.0.1:4319
```

- `CODEMOD_BIN=<path>` points the server at another binary; `--port <n>` changes the port.
- Without the dashboard:
  `target/debug/codemod workflow run -w workflow-lab/workflows/react-audit/workflow.ts --target ~/lab-repos/react-apps --events /tmp/audit.events.jsonl`
- Runs are written to `workflow-lab/.runs/` (gitignored).

## Decisions waiting on Dev

1. **Port or archive.** Port the three workflows and the dashboard to #2485's engine, or keep
   this branch only as a record. My recommendation: port. Most of the dashboard's server logic
   can then be dropped in favour of `codemod workflow list/status/run --format jsonl` and the run
   store.
2. **Where the dashboard lives.** Keep a small local server (Node, or served by the CLI), or
   build it into codemod-app. Dev's Nov 11 dashboard designs are at
   https://claude.ai/artifact/M5CH9dS5R75FLLHpUJA7Xh (weekend, proper, advanced); a pick was
   still open.
3. **What to build next on the engine.** The Nov 11 question (retrying resume, a CLI run store,
   or registry codemods as steps) is answered by #2485, which has all three. What's left from
   the list is workflow metadata/result shape (4), and checking 8 to 10 on #2485.
4. **This branch's `--events` change.** It only matters if #2411's engine continues. Otherwise
   drop it.

## Next steps

1. Start a branch from `engine/code-first` (or from #2491 if its jssg `data` and shell
   `stdout`/`exitCode` are needed, which they are for the audit and the act migration).
2. Port the workflows to `codemod/workflow`:
   - `react-audit`: `forEachFile` or `jssg` over each repo returning findings as `data`;
     repo info from a shell step's `stdout`.
   - `react-19-readiness`: same, with a `json` transform for `package.json`.
   - `react-19-act-migration`: try `codemod("react/19/replace-act-import")` instead of the
     inline rewrite, then the before/after count and the shell verify and commit steps.
3. Re-point the dashboard at the CLI: read runs from the run store (`workflow list`, `status`,
   or the run directories), stream `--format jsonl` from the child process instead of the events
   file, and use `workflow cancel` and `workflow resume` instead of SIGTERM and history editing.
4. Re-run the four checks above (audit counts, readiness counts, act before/after, cancel then
   resume) on the ported versions and update PRIMITIVES.md with what #2485 still lacks.
5. Ask Dev before opening any PR or touching Linear.

## Where things are

- Branch: https://github.com/codemod/codemod/tree/claude/project-thread-xa4azn
- Lab: `workflow-lab/` (README, PRIMITIVES, workflows, dashboard, `setup-repos.sh`, screenshots).
- Related notes in the shared project folder: `/mnt/project-files/v2-react-to-solid/ENGINE-GAPS.md`
  (engine gaps found on #2485), `/mnt/project-files/v2-returned-data/NOTES.md` (#2489),
  `/mnt/project-files/review-2026-11-17/pr-2491.md` (review of #2491),
  `/mnt/project-files/jssg-audit/` (source of the audit detectors).

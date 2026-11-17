# Handoff: agent alone vs agent + Solid migration assistant

Branch `handoff/agent-vs-sma` in `codemod/codemod`, written 2026-11-17. This is an orphan branch
that holds only the experiment's scripts, prompts and graded results. It is not code to merge.

## Question

Does giving a coding agent the Solid migration assistant (`devagrawal09/solid-migration-assistant`,
branch `battle-test-fixes` at c1db086, now `handoff/sma-quick-release`) make a real Solid 1.9 to
Solid 2 migration faster, cheaper or more reliable than the same agent working from the official
migration guide alone?

## Answer (3 runs per arm)

| Arm | Median wall time | Median cost | Median tsc runs | Reached the finish line |
|---|---|---|---|---|
| Agent alone | 23.1 min | $9.02 | 56 | 2 of 3 |
| Agent + assistant | 19.7 min | $7.35 | 37 | 1 of 3 |

- The assistant arm was somewhat faster and cheaper, with fewer type-check loops.
- It did **not** finish more often. The spread inside each arm is as large as the gap between arms,
  so three runs per arm can't support a "more reliable" claim either way.
- The two arms fail on the same thing: `packages/ui` typing around `Dynamic`, which neither the
  guide nor the assistant covers. That's the most useful follow-up for the assistant.
- About $53 was spent in total across the six valid runs (plus one discarded launch).

Full table and grading notes: `RESULTS.md`.

## Setup

- Target: codeimage (pnpm monorepo, public), commit 27b185f, Solid 1.9.
- Agent: Claude Code headless (`claude -p`), model `sonnet`, $24 budget cap per run, 9000 s timeout.
  No budget cap was hit.
- Both arms get the same prompt (`prompts/base.md`) and the official guide from the solid-js 2.0.0
  source (`MIGRATION-solid-2.0.0.md`). The assistant arm also gets `prompts/assistant.md` and the
  `solid-migration-assistant` command on its PATH.
- Allowed tools: file edits plus `pnpm` (and `solid-migration-assistant` in the assistant arm). No web.
- Scope: core Solid only. Twelve libraries with no Solid 2 release are named in the prompt and left alone.

## Files

| Path | What it is |
|---|---|
| `run.sh <arm> <n>` | One fresh checkout, one agent run, then grading. Paths are the original container's (`/home/user/scratch/avs`, `/mnt/project-files/agent-vs-sma`); change `AVS` and `OUT` to rerun. |
| `prompts/base.md`, `prompts/assistant.md` | The exact prompts. `DOCS_DIR` is replaced at run time. |
| `MIGRATION-solid-2.0.0.md` | The guide both arms read. |
| `baseline-solid1.json` | tsc errors that already exist on Solid 1 (15), excluded from grading. |
| `grade.py` | Checks every Solid package declares solid-js 2 and counts tsc errors as baseline, library or core. |
| `summarize.py`, `peek.py` | Read the transcripts into the per-run table, and peek at a running run. |
| `runs/<arm>-<n>/` | `grade.json`, `wall-seconds`, `agent-exit`, and `files-changed.txt` (count plus file list of the agent's diff). |

## What's deliberately not here

- The transcripts (`transcript.jsonl`, about 5 MB each) and the agents' full diffs (`changes.diff`,
  about 250 KB each, edits to a third-party app). They stay in the project's shared folder at
  `/mnt/project-files/agent-vs-sma/runs/`. That folder is not durable forever, so copy them somewhere
  if you want them later.
- `invalid-first-launch/`: a first launch that was discarded because a permission setup let some
  edits fail.

## Known grading gaps

- `grade.py` only traces errors to libraries through direct imports. Eight errors appear in every run
  through local wrappers around Kobalte, solid-toast, @codeui/kit or statebuilder; `RESULTS.md` counts
  them as library errors by hand, but each `grade.json` still lists them as core (so its `finished`
  flag is `false` for every run). Trust `RESULTS.md` over the raw `grade.json` files.
- The prompt missed three libraries with no Solid 2 release (statebuilder, @codeui/kit, @corvu/utils).
  Both arms had the same prompt, so the comparison is still fair.

## Decisions waiting on Dev

1. Whether this result is worth publishing (case study or blog). As it stands it supports "slightly
   faster and cheaper", not "more reliable".
2. Whether to spend more (roughly $9 per run) on more runs per arm, or first fix the `Dynamic` gap in
   the assistant and rerun.

## Next steps if picked up

1. Teach the assistant the `packages/ui` `Dynamic` typing change, then rerun both arms.
2. Fix `grade.py` to follow local wrapper modules so `grade.json` matches the hand grading.
3. Run 5+ per arm before making any reliability claim.

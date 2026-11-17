# Handoff: codebase-roaster as a workflow v2 workflow

Branch `handoff/codebase-roaster-v2` in `codemod/codemod`, written 2026-11-17. Orphan branch that
holds the workflow package, results and patches. Nothing here was pushed anywhere before this.
Read `DESIGN.md` next: it is the full write-up from Nov 16.

## Goal

A real-world proof for workflow v2 (Mo's code-first engine, `engine/code-first`, #2485): rewrite the
`codebase-roaster` CLI (a static "roast your codebase" report built from ast-grep slices) as a v2
workflow, and check it gives the same findings, faster.

## Result

- Same findings as the original CLI on two codebases (only the random `Roast:` lines differ).
- About 3x faster: codemod/codemod at 205d920 (475 files) went from 86.1 s with the CLI to 26.6 s
  with one step per slice family, or 23.4 s with a single step. The per-family split is Dev's pick;
  it costs about 12% wall time.
- 5 `node:test` specs through `codemod/testing`'s `runWorkflow` pass on the Node host. The QuickJS
  host was not run.

## State

- Built on `codemod@2.0.0-alpha.3` (engine 9c91ebf). It still uses `console.log` and returns findings
  from `forEachFile`, so it doesn't touch workflow state or metrics, but it has not been rerun on the
  engine after #2489 (state removed) and #2491 (step results reshaped). Expect small API updates.
- The engine bug it found (`codemod workflow validate -w src/workflow.ts` can't resolve
  `./core.ts`) was fixed in #2488, which Mo merged into `engine/code-first`.
  `patches/engine-0001-resolve-relative-workflow-path.patch` is the original local version of that fix;
  it's here for the record only.
- `patches/0001..0003` are the history of the `workflow/` folder against the roaster repo. They are
  authored as Claude; they were never pushed, so rewrite the author if they ever go to a public repo.

## Files

| Path | What it is |
|---|---|
| `DESIGN.md` | Full write-up: result table, shape of the port, engine notes for Mo |
| `workflow/` | The v2 package (`src/workflow.ts`, `src/core.ts`, `tests/workflow.test.ts`, `codemod.yaml`). It imports the slices unchanged from the roaster repo's `src/slices`, so it must sit inside a roaster checkout. |
| `results/` | CLI vs v2 reports for both targets, and the single-step variant used for timing |
| `patches/` | Roaster history (0001 to 0003) and the original engine fix |

Run: `npm i` in `workflow/`, then `codemod workflow run -w $PWD/src/workflow.ts -t <repo>`.

## Decisions waiting on Dev

1. Whether to keep this as a case study for the v2 blog post or docs (it's a clean "same findings,
   3x faster" proof), and where the roaster's v2 package should live.
2. Whether to port it to the current engine (after #2489 and #2491) now, or wait until v2 is on npm.

## Next steps if picked up

1. Rebase onto the current `engine/code-first`, rerun the specs and both timing targets.
2. Run the specs on the QuickJS host too.

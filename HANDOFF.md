# Handoff: benchmark data behind the workflow v2 blog post

Branch `handoff/blog-workflow-v2-bench` in `codemod/codemod`, written 2026-11-17. Orphan branch with
the benchmark scripts and raw results used for the numbers in the workflow v2 blog post
(codemod/codemod-app #1418, draft, `apps/website/src/content/blog/migration-workflows-as-code.mdx`).
The post itself is only on that PR; this branch keeps the data that backs it.

## What was measured

Same machine, shuffled arm order per round, repo reset before each run (`run.py`). Each row records
wall time, CPU, peak RSS, exit code and a hash of the resulting diff, so arms can be checked for
identical output.

| File | Workloads | Arms |
|---|---|---|
| `engine.csv` / `.out` | no-op probe, the jssg audit pack on excalidraw and cal.com, and other cells in `cells.py` | `v1` (YAML engine on main) vs `v2-yaml` (v2 running the translated YAML) and v2 TS where present |
| `sma.csv` / `.out` | Solid migration assistant `analyze` / `migrate` on real Solid apps | `v1`, `v2-yaml`, `v2-ts` |
| `micro.csv` / `.out` | jssg step vs `forEachFile` callback, and similar micro cases | v2 variants |

`bench/` is the harness from the TS orchestration proof (see `handoff/ts-orchestration-proof`),
reused for setup and the ported workflows.

## Known caveat

These runs show v2 using far more memory than v1 on large repos (for example about 590 MB vs 60 MB
for the audit on cal.com). That is the question Dev owes Mo an answer on; the follow-up perf work
(memory fix 600 to 190 MB on cal.com, worker pool) is on `claude/project-thread-bffawz` with
`/mnt/project-files/handoff/engine-perf.md`. If the post quotes memory, recheck it against that work
before publishing.

## Decisions waiting on Dev

1. Rerun these cells on the engine as it will ship (after #2489, #2491 and any perf fixes) before
   #1418 is published, so the post's numbers match the release. The post waits on v2 being on npm.

# Handoff: TypeScript orchestration proof (simplicity and performance)

Branch `handoff/ts-orchestration-proof` in `codemod/codemod`, written 2026-11-17. Orphan branch that
holds the evidence pack, benchmark harness and local patches from Nov 8 to Nov 10. It is a record,
not active work: read "Where this stands" first.

## Where this stands

- Goal (Nov 8, Dev's words): replace the YAML workflow engine with the TypeScript orchestration
  prototype; "i need stronger proof... of simplicity and performance". This folder is that proof.
- The prototype is `prototype/typescript-orchestration` (draft PR #2411, now closed). Its code, including the
  QuickJS-only work and the patches here, is already on that branch on GitHub (head b71f40b).
- **Since then the direction changed.** Mo's code-first engine (`engine/code-first`, #2485, published
  as `codemod@2.0.0-alpha.3`) became workflow v2, and work moved there. The useful carry-overs:
  - The benchmark harness in `bench/` is what later measured v2 (the blog post's numbers, and the
    current engine-perf branch `claude/project-thread-bffawz`).
  - The transform worker pool (patch 0003) is the model for the v2 worker-pool work.
  - `RUST-CLI-PLAN.md` (fold the TS CLI into the Rust `codemod` CLI, no Node) matches Dev's Nov 8
    call: "lets get rid of node", "cli also gets rewritten in rust".

## Headline results (Nov 9, 4 vCPU)

- With the 4-worker transform pool, TS beat YAML on every non-trivial workload: react-19 on
  excalidraw 7.1 s vs 12.7 s; debarrel on cal.com 17.0 s vs 33.5 s; the audit pack on cal.com
  13.2 s vs 78.8 s. Diffs byte-identical, same audit findings, 1.3 to 5.6x less CPU.
- Simplicity: TS workflows shorter in 3 of 4 ports, orchestration core 41% smaller, 7 functions to
  learn instead of 35 schema types, `tsc` catches 5 of 6 wiring mistakes YAML accepts.
- Parity gap estimated at 60 to 95 engineer-days of blockers plus 30 to 55 of major gaps.

## Files

| Path | What it is |
|---|---|
| `REPORT.md` | The full proof: verdict, perf tables, simplicity, parity gaps |
| `PR-2411-TOUR.md` | A reviewer's tour of #2411 |
| `RUST-CLI-PLAN.md` | Plan to fold the TS workflow CLI into the Rust CLI (not started) |
| `ROLLDOWN-SPIKE.md`, `perf-rolldown/` | Spike: Rolldown instead of esbuild for bundling |
| `bench/` | Benchmark harness (`setup.sh`, `run-all.sh`, `run-pool.sh`, `analyze.py`) and the four ported workflows (util-is, react-19, debarrel, audit), YAML and TS side by side |
| `perf/`, `perf-pool/`, `perf-rust-build/` | Raw timings and memory (CSV) for each pass |
| `simplicity/`, `spec-rewrite/`, `*-tests-before/after.txt` | Line counts, mistake catalogue, and the spec-style test rewrite outputs |
| `patches/ts-orch/` | The local patch series on #2411 (0001 to 0018). Already on the PR branch; kept for the record. |
| `patches/yaml-signal/` | Bench-only cherry-pick of the YAML completion-signal fix (the fair baseline) |

Left out on purpose: the 2 MB git bundle and the 1.5 MB "merge main" diff (both reproducible from the
PR branch).

## Decisions waiting on Dev

None. #2411 is already closed; its branch stays as the reference. Nothing on it is needed for v2.

## Next steps if picked up

None for this branch itself. Reuse `bench/` for any further v2 performance claims.

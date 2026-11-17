# Codebase roaster on workflow v2 (Nov 16)

Runs on codemod 2.0.0-alpha.3 (Mo's engine/code-first at 9c91ebf). Local only: nothing pushed.

## Result
On two public codebases the v2 workflow reports exactly the same findings as the original
`codebase-roaster` CLI (only the random `Roast:` lines differ), and it is about 3x faster.

| Target (files) | Original CLI | v2, one step per family | v2, one step for everything |
|---|---|---|---|
| roaster itself (git archive of 5d4d4d1) | 6.0s | 1.9s | not timed |
| codemod/codemod main 205d920 (475 JS/TS/Rust files) | 86.1s | 26.6s, 25.8s | 23.4s, 23.4s |

Same machine, 4 cores, one run each for the CLI. Diff of the reports with `Roast:` lines removed:
identical apart from the engine's trailing `Done`. Outputs are in `results/`.

The per-family split (Dev's pick) costs about 12% wall time against a single step on this repo:
each file is parsed 9 times, but the 9 steps run side by side.

## Shape
| Roaster today | v2 workflow |
|---|---|
| discovery (gitignore, skip node_modules/dist/build/coverage) | `forEachFile({ include: 9 extension globs, exclude: generated dirs })`; the engine walk respects .gitignore |
| parseFile + read-only SyntaxNode adapter | same adapter over `parse(file).root()` from `codemod/ast-grep` |
| analyze every slice per file | one `forEachFile` step per family (complexity, coupling, reliability, rust, sensitive, state, structure, testing, types), run together with Promise.all |
| merge + report | in the workflow body (pure, re-runs on resume) |
| random quip | `Math.random` in the body, seeded per run: random across runs, same on resume |
| print to stdout | `console.log` in the body (it lands on stderr with the step progress) |
| parse warnings | forEachFile `failures` rendered as coverage warnings |

Layout follows the alpha.3 `init` scaffold: `workflow/src/workflow.ts`, `workflow/src/core.ts`,
`workflow/tests/workflow.test.ts`, `workflow/codemod.yaml` (`workflow: src/workflow.ts`).
Slices are imported unchanged from the roaster's `src/slices`.

Tests: 5 node:test specs through `codemod/testing`'s `runWorkflow` on the Node host, all pass:
empty catch reported, clean file gives no findings, node_modules skipped, Rust unwrap density
reported, one step per family. Not run: the quickjs host.

## Engine notes for Mo
1. Bug, fix in PR #2488 (https://github.com/codemod/codemod/pull/2488): `codemod workflow validate -w src/workflow.ts`
   fails with "cannot resolve './core.ts'". `check_imports` in `crates/workflow/src/build.rs`
   passes the relative path to the resolver, which joins its parent onto a base dir that is
   already that parent (`src/src/core.ts`). This breaks the `validate` script that `init` itself
   scaffolds (`workflow run` with the same path works). The fix resolves from the absolute path;
   a new test in `tests/engine.rs` passes, and a local build validates `src/workflow.ts` and
   `workflow/src/workflow.ts` and gives the same report.
2. alpha.3 still prints `codemod 1.18.7` for `--version` (so did alpha.1).
3. alpha.1 (gitHead 205d920, main) has no workflow.ts engine.
4. `jssg` steps return no per-file outputs, so findings can't come from jssg steps.
5. A rule naming a kind the grammar lacks (catch_clause on Rust) throws per file; the roaster's
   language filter avoids it.

## Files
- `workflow/`: the package. Install with `npm i`, run with
  `codemod workflow run -w $PWD/src/workflow.ts -t <repo>`.
- `patches/0001..0003`: roaster history (4caf9ed bundle version, 5a33930 port, alpha.3 move).
- `patches/engine-0001-resolve-relative-workflow-path.patch`: the engine fix, uncommitted.
- `results/`: CLI and v2 reports for both targets, and the single-step variant used for timing.

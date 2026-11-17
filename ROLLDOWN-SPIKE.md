# Spike: rolldown + oxc in place of esbuild for the QuickJS bundle

Spike code: `/home/user/spike-rolldown` (`rs/src/main.rs`, ~290 lines; rolldown
=1.3.0, oxc =0.157.0). Benchmark: `perf-rolldown/rolldown.csv` and `logs/`.

## What it does

For each of the 4 ported workflows (util-is, react-19, audit, debarrel) it
builds, in Rust:

- every `jssg()` transform artifact (21 in total), from the same entry esbuild
  gets, with `codemod:*` left external;
- the workflow bundle, with the resolver rules of `quickjs-bundle.ts`:
  `@codemod.com/orchestration` -> `portable.ts`, `node:*` and deep package
  imports rejected with the same messages, async functions and generators
  lowered for QuickJS (oxc `async_to_generator`).

The rolldown bundles were swapped into the bundle cache and run by the normal
CLI and QuickJS host against the esbuild bundles (same harness as the earlier
benchmarks, arms interleaved).

## Results

| | esbuild | rolldown + oxc |
| --- | --- | --- |
| Bundle build, warm (workflow + artifacts) | 33-71 ms | 27-52 ms |
| Bundle build, first in process | 371 ms (TS + esbuild load) | same as warm, no runtime to load |
| Bundle size | 182-365 KB | 6-8% larger (keeps comments; no minify set) |
| Remaining `async`/`await` in workflow code | 0 | 0 |

Run results, median wall time:

| Cell | esbuild | rolldown | Same edits? |
| --- | --- | --- | --- |
| react-19 on excalidraw (5 runs) | 7.20 s | 7.37 s | yes |
| audit on excalidraw (5 runs) | 4.84 s | 4.94 s | yes, identical findings output |
| debarrel on cal.com (3 runs) | 18.66 s | 18.02 s | yes, same 188 files, same diff hash |
| util-is on bluebird (3 runs) | 0.28 s | 0.26 s | yes |
| all 4 on probe and taxonomy (1 run) | | | yes, including the 1-file edits on probe |

Run time differences are within run-to-run noise; peak memory is identical.
The bundler only changes how the bundle is built, not how fast it runs.

## What's missing before it can replace esbuild

1. **The `jssg()` split still uses the TypeScript compiler** (`bundle/build.ts`):
   the spike consumed its rewritten output. It needs a port to oxc (find the
   calls, reject captured module bindings, cut the transform's source, rewrite
   to `{ name, hash }`). This is the biggest piece.
2. **oxc's async helpers are loaded from `node_modules`**; they need vendoring
   (or the runtime pre-bundled once, see the plan).
3. **Bundle hashes change**, so cached bundles rebuild once and recorded
   artifact hashes in old histories won't replay across the switch.
4. Comments are kept; turn them off or minify if bundle size matters.

## Recommendation

Use rolldown + oxc, as part of the Rust CLI, not as a swap inside the TS CLI.
It produces bundles that behave identically, builds them a little faster, and
removes the last reason to load Node and TypeScript to run a workflow. On its
own inside the Node CLI it buys almost nothing, since warm runs already skip
the build.

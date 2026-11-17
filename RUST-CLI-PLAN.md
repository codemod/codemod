# Folding the TS workflow CLI into the Rust `codemod` CLI

Plan only. Nothing here is ported yet. Branch state: `local/ts-orch-quickjs-only`
(Node runtime removed, quickjs only, model-agnostic assessments).

## Where Node still is today

At run time the whole workflow already runs in Rust: the control plane,
the workflow body, every transform and every operation executor are bundled
into one ES module and evaluated by `orchestration-quickjs-host` in a single
QuickJS context. Node is only the launcher:

| Node-side file | What it does | Lines |
| --- | --- | --- |
| `bin/codemod-workflow.mjs`, `bin/register-ts.mjs` | entry, TS loader | small |
| `src/host/cli.ts` | args, signals, `run` / `build` | 198 |
| `src/host/runtimes/{quickjs,registry,agent-bin,types}.ts` | find the host binary and `codemod-agent`, spawn the host, parse `{ ok, output }` | ~330 |
| `src/host/quickjs-bundle.ts` | esbuild bundle of workflow + portable runtime, async lowering, bundle cache | 262 |
| `src/bundle/build.ts` | TypeScript-compiler pass that finds `jssg({ transform })`, rejects captured module bindings, bundles each transform as an artifact, rewrites the call to an `ArtifactRef` | 551 |
| `src/bundle/cache.ts` | private cache dir, atomic writes | 79 |
| `src/host/timings-file.ts`, `node-host.ts`, `harness.ts` | bench timings; the Node `Host` used by the in-process test harness | ~420 |
| `src/host/dashboard/monitor.ts` + `ui.html` | run model behind the dropped `--dashboard` | 414 |

Everything else under `src/` (about 8,000 lines: authoring, core, execution,
runtime, `host/quickjs/*`) runs inside QuickJS and stays TypeScript.

## Target shape

`codemod workflow run workflow.ts --target <dir> [--input <json>] [--history <file>]`
in the existing Rust CLI. No Node at run time, no `node_modules` needed to run
a workflow.

1. **Embedded runtime.** At CLI build time (a `build.rs` step), bundle the
   portable runtime (`portable.ts` + `host/quickjs/entry.ts`) once, async
   lowered, and `include_str!` it. Workflow bundles import it as a virtual
   module through the host's existing `ModuleRegistry` instead of re-bundling
   ~45 runtime modules per workflow. This also removes the need to ship the
   orchestration TS sources with the binary.
2. **Workflow build in Rust** (`crates/orchestration-build`, new):
   - rolldown + oxc bundle of the user's workflow, with the resolver rules
     from `quickjs-bundle.ts`: `@codemod.com/orchestration` -> the embedded
     runtime, `node:*` and deep package imports rejected with the same
     messages, `codemod:*` external. The spike reproduces all of these.
   - Port of `bundle/build.ts` to oxc: find `jssg()` calls with
     `oxc_semantic`, reject transforms that capture module-level bindings
     (same error positions), cut each transform's source span, bundle it as an
     artifact, rewrite the call to `{ name, hash }`. oxc's scoping makes the
     capture check simpler than the TS-compiler version.
   - Cache: same manifest (builder id + every input file's hash), keyed in
     the CLI's existing cache dir (`commands/cache.rs` already manages one).
3. **Host linked in.** `orchestration-quickjs-host` is already a library
   (`run(HostOptions)`); the CLI calls it in-process instead of spawning a
   binary. Same rquickjs 0.11 as `codemod-sandbox`. `codemod-agent` is found
   next to the CLI binary as today.
4. **CLI surface.** `workflow run` picks the engine by file: `.ts`/`.mts` ->
   new engine, `.yaml` -> butterflow (until YAML is retired). `--history`
   gives resume; `workflow status` / `list` / `cancel` read the history file.
   `codemod run <package>` uses the new engine when the package ships a
   `workflow.ts`. `jssg run` is unaffected.
5. **Dashboard event stream** (when it comes back): the runtime already emits
   `RunEvent`s (`core/events.ts`) to an `EventSink`. Add a host primitive that
   forwards each event as NDJSON over a channel to the CLI; the CLI serves
   `ui.html` and an SSE endpoint, and ports `monitor.ts`'s projection
   (snapshot + event list, ~400 lines) to Rust, or keeps the projection in
   QuickJS and streams snapshots. The second is less code; the first keeps the
   UI working when the workflow context is busy.

## What moves, what is deleted

- **Deleted:** `bin/*.mjs`, `host/cli.ts`, `host/runtimes/*`,
  `host/quickjs-bundle.ts`, `bundle/build.ts`, `bundle/cache.ts`,
  `host/timings-file.ts`, the `esbuild` and `typescript` runtime dependencies,
  and the separate `orchestration-quickjs-host` binary (the crate stays as a
  library). About 1,400 lines of TS.
- **Moved to Rust:** the bundle/resolve rules, the `jssg()` split and capture
  check, the bundle cache, arg parsing and signal handling, host-binary and
  agent-binary discovery (mostly disappears), bench timings.
- **Stays TS (runs in QuickJS):** authoring, core, execution, runtime,
  `host/quickjs/*`. Tests for these keep running under vitest with the
  in-process harness; Node stays a dev dependency for those tests only.
- **Not in this step:** retiring butterflow. `butterflow-core` and friends are
  ~36,000 lines and the CLI imports them from 30 files (registry, publish,
  auth, `jssg run`, cache, TUI), so removing the YAML engine is its own plan
  once registry packages can ship `workflow.ts`.

## Tests that move

| Today (vitest) | Cases | Becomes |
| --- | --- | --- |
| `build.test.ts`, `build-cache.test.ts`, `quickjs-bundle.test.ts` | 16 | Rust unit tests in `orchestration-build` (same fixtures) |
| `executor.test.ts`, `runtime.test.ts` (CLI parts) | ~15 | `assert_cmd` tests on `codemod workflow run` |
| `quickjs.e2e.test.ts`, `jssg-parity.e2e.test.ts`, `timings.e2e.test.ts` | ~30 | Rust integration tests driving the CLI on fixtures |
| `timings.test.ts`, `dashboard.test.ts` | 20 | with their code (timings now; dashboard when it returns) |
| everything else (the portable runtime's specs) | | stays vitest |

## Estimate

| Step | Size |
| --- | --- |
| Embedded runtime bundle + virtual-module import | 1-2 days |
| `orchestration-build`: rolldown/oxc bundle + resolver rules (spike is ~290 lines) | 1-2 days |
| Port `bundle/build.ts` (jssg split, capture check, rewrite) | 2-3 days |
| Cache + `workflow run` integration, signals, `--history` resume | 2 days |
| Move the ~60 Node-side tests to Rust, CI wiring | 2-3 days |
| **Total** | **about 8-12 working days**, dashboard excluded (+2-3 days) |

Main risks: rolldown's and oxc's Rust APIs change between versions (the
spike pins rolldown =1.3.0 and oxc =0.157.0, and upgrades will need small
fixes); oxc's async-lowering helpers need vendoring; bundle hashes change, so existing caches and recorded
artifact hashes in histories won't replay across the switch (one-time).

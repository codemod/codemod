# Runtimes and the host seam

> Status: prototype. One runtime, `quickjs` (the default and only one), run by
> `codemod workflow run -w` in the `codemod` process: the QuickJS host is a library crate,
> `crates/orchestration-quickjs-host`; see "The quickjs runtime". `agent` steps also need the
> `codemod-agent` runner (`cargo build -p orchestration-agent`), checked only when one runs.
> The `bridge` runtime (one Rust process per command, `crates/execution-bridge`) was removed on
> 2026-11-08, and the `node` runtime (the workflow in one Node process, transforms over a napi
> addon, `crates/jssg-napi`) after it. On 2026-11-09 the Node launcher
> (`bin/codemod-workflow.mjs`) and the separate host binary were removed as well.

```text
codemod workflow run -w <workflow.ts|.mts> [--target <dir>] [--input <json>] [--history <file>]
run(executable, { executor, runtime: QUICKJS_RUNTIME }) // any host, incl. QuickJS
createHarness({ runtime: "quickjs" })                  // tests: Node, scripted executors
```

| Runtime | Control plane | File selection + reads | `jssg` | `shell` | `assessment` | `agent` | Semantic analysis |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `quickjs` | QuickJS (in the `codemod` process) | native (host primitives, `orchestration-walker`) | in-process in the same context | Rust host spawn | decision adapters, `fetch` over `httpRequest` (decision API root only) | `codemod-agent` runner via the host's spawn | yes (batch provider, `jssgIndex`) |

`quickjs` executes every operation kind and applies the JSSG batch rules of
`crates/jssg-batch` (planning, containment, semantic provider, workspace index, selector
skip, result conversion) to verbatim sources ("Source encoding"). `codemod workflow run -w`
is one Rust process: it builds or reuses the workflow bundle (`crates/orchestration-build`,
cached by content), then runs it on the QuickJS host in-process. Node is not involved.
Besides `shell` commands, the only other processes are `codemod-agent` runners, one per
`agent` step.

## Files

| Path | What |
| --- | --- |
| `src/core/host.ts` | `Host` interface, `setHost`/`getHost`/`installDefaultHost`, `HOST_GLOBAL` |
| `src/core/posix.ts` | dependency-free port of `node:path.posix` (`createPosixPath`) |
| `src/host/node-host.ts` | Node `Host` (`NODE_HOST`, `createNodeHost`); importing it installs it |
| `src/execution/operation-executor.ts` | `OperationExecutor` (moved out of `executor.ts`) |
| `src/execution/jssg.ts` | portable `executeJssgCommand({ ..., dispatch, host })` |
| `src/execution/shell.ts` | portable `executeShell` with DirectRunner semantics |
| `src/execution/assessment.ts`, `src/execution/decision/` | portable `executeAssessmentOperation(options, cwd, request, signal)` over the provider adapters, the HTTP transport, and the `Host` fs; settings from `src/core/decision.ts` |
| `src/execution/agent.ts`, `src/execution/agent-env.ts` | portable `executeAgent`: one `codemod-agent` process per step through `Host.spawn`, allowlisted environment (`agentLaunch`), key on stdin |
| `crates/cli/src/commands/workflow/typescript.rs` | `codemod workflow run -w`: builds or reuses the bundle, runs it on the host in-process, prints the output as JSON; sets `CODEMOD_AGENT_BIN` to the `codemod-agent` next to the binary when unset |
| `crates/orchestration-agent` | the `codemod-agent` binary: one `agent` operation, request on stdin, completion on stdout (`PROTOCOL.md`) |
| `src/execution/jssg-batch.ts` | the portable JSSG batch loop, over a `JssgBatchEngine` (see "JSSG batches in-process") |
| `crates/jssg-batch` | the JSSG batch rules: `BatchPlan` (planning, containment, selector skip, parse, result conversion) |
| `src/runtime/capabilities.ts` | runtime descriptors, `CapabilityExecutor`, `assertRuntimeSupports` |
| `src/host/harness.ts` | `createHarness`: runs workflows in Node with scripted executors, for tests |
| `crates/orchestration-walker` | native `walk()` of `execution/files.ts` + batched source reads (`Host.selectFiles`, `Host.fs.readSources`) |
| `crates/orchestration-build` | the build step: the `jssg()` split (oxc, `src/split.rs`), rolldown bundles (`src/bundle.rs`), the content-keyed cache (`src/cache.rs`, `$CODEMOD_WORKFLOW_CACHE`), the runtime sources embedded at build time (`build.rs`, `src/runtime.rs`; `CODEMOD_ORCHESTRATION_SRC` overrides them) |
| `src/portable.ts` | `@codemod.com/orchestration/portable`: the Node-free API |
| `tests/portable.test.ts` | static import scan of everything reachable from `portable.ts` and `host/quickjs/entry.ts` |
| `src/host/quickjs/{entry,host,dispatch,async-context,native,fetch}.ts` | runs inside QuickJS: bundle entry, `Host`, the in-context `JssgBatchEngine`, async context, the Rust primitives' types, the assessment `fetch` over `httpRequest` |
| `crates/orchestration-quickjs-host` | the host library the CLI calls in-process: one runtime + one context, Rust primitives, JSSG per-file calls, the transform pool (`src/pool.rs`), `httpRequest` (`src/http.rs`) |
| `crates/codemod-sandbox/src/sandbox/engine/embedded.rs` | JSSG in an embedder-owned context: built-in modules, userdata, `begin_call`/`end_call`/`transform_output` |

## The quickjs runtime

```text
codemod workflow run -w wf.ts --target R
  Rust: cached_quickjs_bundle(wf.ts)   (oxc + rolldown only on a miss; a hit hashes the
                                        inputs and the bundle, nothing else; see README "Startup")
  orchestration_quickjs_host::run(bundle, target R), in the same process
    Rust: AsyncRuntime + ONE AsyncContext; transform built-ins (codemod:ast-grep over sg_core, ...)
          + LLRT globals; globalThis.__codemodQuickjsNative (fs, env, clock, spawn, jssg*)
    JS:   bundle entry installs Host + async context; run(workflow, { executor, runtime: QUICKJS_RUNTIME })
          jssg  -> executeJssgCommand({ dispatch }) -> the shared batch loop (jssg-batch.ts):
                   jssgBegin (Rust BatchPlan) -> [workspace: jssgIndex every file] -> per chunk
                   jssgParse (Rust parse + selector) -> per file jssgCall / transform(root,
                   options) (artifact module, evaluated once per run) / jssgCallEnd ->
                   jssgFinish (Rust, BatchPlan::finish) -> select/stage/commit unchanged
          shell -> executeShell over Host.spawn (Rust: own process group, merged pipe, tree kill on abort)
          assessment -> executeAssessmentOperation (decision adapter, fetch over httpRequest)
          agent -> executeAgent -> Host.spawn codemod-agent (stdin request + key, stdout completion)
```

- **Bundle** (`crates/orchestration-build`, written to the cache): the entry, the
  portable closure, the workflow graph rewritten by the build step (`src/split.rs`: each
  inline transform becomes an `ArtifactRef`, so command identities and history are the
  transforms' content hashes), and every artifact's
  source verbatim, keyed by hash. `@codemod.com/orchestration` (and the package's
  `src/index.ts`) resolve to `src/portable.ts`; `node:*` fails the build; `codemod:*` stays
  external. Async is lowered (see "Async context").
- **Transforms** are evaluated once per run per artifact hash (`import()` of a registered
  virtual module), so module-level state is shared across files and commands of a run:
  unsupported. Each file gets an `SgRoot`, `options` (`params.input`, `language`,
  `targetDir`, `dryRun: false`), string/null/`{ content, output }` handling, `rename()`,
  staged `jssgTransform`, and path validation from the shared Rust code
  (`crates/jssg-batch`). In the bundle's context, transform calls run one at a time across
  the run; files serially. A batch of 64 files or more runs in chunks on the host's
  transform pool (`src/pool.rs`): `CODEMOD_JSSG_WORKERS` threads, else the core count
  capped at 4, each with its own QuickJS context, so there module-level state is per worker.
- **Assessment:** the decision adapters and transport in the bundle, with a `fetch` over the
  host's `httpRequest`, which reaches only the decision API root (`DECISION_BASE_URL`, else
  `TYPESAFE_BASE_URL` for `systemone`, else the provider's default; see `src/core/decision.ts`).
- **Semantic analysis:** the batch's provider (`BatchPlan`), workspace index built by
  `jssgIndex` before the first transform.
- **Agent:** one `codemod-agent` runner process per step (`crates/orchestration-agent`,
  `execution/agent.ts`) through the host's `spawn`: allowlisted environment, the key on
  stdin, the completion on stdout; `CODEMOD_AGENT_BIN`, which the CLI sets to the
  `codemod-agent` next to its own binary when unset. Cancelling kills the runner's process tree, including children that left its
  process group.
- **History/resume:** `codemod workflow run -w wf.ts --history h.json` resumes from
  `h.json` when it exists and writes the run's history back, success or not.
- **Cancellation:** SIGINT/SIGTERM/SIGHUP to the `codemod` process abort the run (a child's process group
  is killed, and descendants that left it: the group is stopped, one `ps` snapshot finds them,
  they are killed, then the group, as Node's `killProcessTree` does). A spinning transform cannot be interrupted (one JS thread). The signal
  watcher is a future on the host's event loop, and rquickjs drains every pending promise
  job before it polls futures again; a JSSG batch awaits only resolved promises (parsing is
  synchronous here), so the in-context engine awaits `yieldNow()` (a `tokio::task::yield_now`
  that lets the driver read signals, child output, and timers) before each 64-file chunk.
  A SIGINT mid-batch therefore cancels the command within one chunk with nothing written
  (before, it was only seen after the batch, and a jssg-only run finished normally).
  `crates/cli/tests/typescript_workflow.rs`
  (`sigint_cancels_a_running_jssg_batch_promptly_and_writes_nothing`) pins it.
- **Console:** workflow and transform `console.*` go to stderr; stdout is the result.
- **Timings:** a `codemod` built with the `timings` feature writes the run's phases, the
  bundle's JS-side phases included, to `CODEMOD_TIMINGS_FILE`.
- **File selection is native.** The host implements the optional `Host.selectFiles` and
  `Host.fs.readSources` (see "Native file selection"), so the walk and the reads of a JSSG
  command run in Rust instead of the interpreter. `CODEMOD_NATIVE_SELECT=0` turns both off
  (the portable walker then runs in QuickJS; same result, slower).

## Native file selection

`execution/files.ts` is the reference walker. A host may also offer two optional
capabilities, and `files.ts` uses them when present:

- `Host.selectFiles(request)`: `walk()` with exactly its semantics. TypeScript still derives
  the inputs (language extensions, brace expansion, include/exclude lines, global excludes
  discovery, building the override matchers, so pattern errors are thrown by the same code)
  and passes `{ root, overrides: [{ root, patterns, whitelists }], globalExcludes }`. It is
  used only for POSIX paths that are absolute, normalized, and nested (each override root is
  the walk root or an ancestor), which `executeJssgCommand` always produces. Anything else
  walks in TypeScript.
- `Host.fs.readSources(root, paths)`: `readText(join(root, path), { fatal: true })` for every
  selected path in one call (`undefined` for `ENOENT`/`EILSEQ`, the first other error thrown).

The QuickJS host implements them as context primitives over `crates/orchestration-walker`
(subdirectories walked and files read on worker threads, results combined in listing order);
the Node host does not, so in Node (the test harness) the TypeScript walker runs. Reads are
verbatim everywhere (see "Source encoding"). `CODEMOD_NATIVE_SELECT=0` turns them off. Both
hosts list directories in byte order of the raw names (Node's libuv sorts `scandir`; the
QuickJS host and the walker sort the same way), so a walk reports the same first failure
everywhere.

The walker is not the engine's walker (`codemod_walk_builder`, the `ignore` crate) because the
TypeScript walker's semantics differ from it in ways only a port reproduces: matching is the
`ignore` npm package, ported rule by rule (`node_ignore.rs`: the same `RegExp` sources,
evaluated by `regress`, an ECMAScript engine, in non-unicode mode over UTF-16 code units, so
case-insensitive, `?` is one UTF-16 unit, `.` stops at line terminators, a rule set JS cannot
compile is dropped whole); a path is ignored when any parent directory is, even if a nearer
`.ignore` whitelisted that directory; two override sets; and `.ignore` / `.gitignore` /
`.git/info/exclude` are read in every ancestor of the target. Paths are the lossy strings the
TypeScript walker sees. Errors carry the host's own `fs` messages (`EACCES: ..., scandir
'<dir>'`) and the `ignore` package's (`path should be a \`path.relative()\`d string, ...`).

`crates/cli/tests/typescript_workflow.rs` (`native_file_selection_matches_the_interpreted_walker`)
checks it differentially: the same workflows run on an awkward tree of nested ignore files
with and without `CODEMOD_NATIVE_SELECT=0` (the TypeScript walk inside QuickJS) and must give
the same result. (`tests/files-native.test.ts`, a broader differential test over the Node host
with a seeded random sweep of ignore files, was deleted with the `node` runtime.)

## Source encoding

JSSG sources reach transforms verbatim, exactly as the workflow engine
(`crates/core/src/jssg_execution_service.rs`, `std::fs::read_to_string`) hands them over:
a leading UTF-8 byte order mark stays in the text as U+FEFF, CRLF stays CRLF, a missing final
newline stays missing, and non-UTF-8 files are skipped. That is the `Host.fs.readText`
contract (the Node host decodes with `TextDecoder(..., { ignoreBOM: true })`; the QuickJS host
and `crates/orchestration-walker` use `String::from_utf8`), so the shared result rules
(`classify_transform_result`) see the same original everywhere. What a transform observes
of a BOM file, identical in `yaml` and `quickjs`:

- tree-sitter starts the root after a leading BOM: `root().text()` has no BOM, but node
  ranges count it (`root().range().start` is `{ line: 0, column: 1, index: 3 }`);
- an edited file is written as the transform returned it, so `commitEdits` output drops the
  BOM; `null` leaves the file (and its BOM) alone; `rename()` with `null` moves the original
  bytes, BOM included; returning `root().text()` unchanged is a modification (it differs from
  the file by the BOM) and rewrites the file without it.

Before this was pinned, the Node host stripped the BOM (`TextDecoder`'s default), so the
Node-hosted runtimes of the time differed from the engine and from `quickjs` in those last
three cases. `crates/cli/tests/jssg_parity.rs` (`the_yaml_engine_writes_the_same_bytes` and
the two encoding tests before it) runs BOM, CRLF, and no-final-newline files through
`quickjs` and through the workflow engine, both in the `codemod` binary under test, and
requires identical written bytes.

## Inventory: what used Node, and where it went

Needed to run a workflow (every operation kind), now behind `Host`:

| Was | Now |
| --- | --- |
| `authoring/context.ts`: `AsyncLocalStorage` | `Host.createAsyncContext()` (created lazily, once per realm) |
| `authoring/target.ts`: `node:path.posix.normalize` | `core/posix.ts` |
| `core/paths.ts`: `lstatSync`, `realpathSync.native`, `node:path` | `Host.fs.lstat/realpath`, `Host.path` |
| `execution/files.ts`: `readFileSync`, `readdirSync`, `homedir`, `node:path`, `process.env`, `languages.json` via `fs` | `Host.fs.readText/readdir`, `Host.os.homedir`, `Host.path`, `Host.env()`, JSON import attribute |
| `execution/jssg.ts`: `node:fs` read/write/mkdir/rm/realpath, `node:path`, `TextDecoder`, direct bridge spawn | `Host.fs`, `Host.path`, `readText(..., { fatal })`, injected `dispatch` |
| `execution/scheduler.ts`: `node:os` `availableParallelism`/`totalmem`, `process.env` | `Host.os`, `Host.env()` (`nodeHost` moved to `host/node-host.ts`) |
| (new) `shell` in-process | `execution/shell.ts` over `Host.spawn`, `Host.fs`, `Host.os.tmpdir`, `Host.randomUUID` |
| `execution/assessment.ts`: `node:fs` reads, global `fetch` | `Host.fs`, `AssessmentExecutorOptions.fetch` (QuickJS: `host/quickjs/fetch.ts`) |
| `execution/agent-env.ts` + the bridge's agent launch | `execution/agent.ts` over `Host.spawn` (`codemod-agent` runner) |

`runtime/run.ts`, `runtime/gate.ts`, `core/history.ts`, and `authoring/composition.ts` used
no Node API directly; they now reach none transitively.

Node-only, stays Node-only (`NODE_ONLY` in `tests/portable.test.ts`):

- `execution/process-tree.ts`: process groups and tree kill behind the Node host's `spawn`.
- `host/**`: the dashboard run model, harness, Node host.

The build step is no longer TypeScript: `crates/orchestration-build` (oxc and rolldown) runs
inside the `codemod` binary.

## The `Host` interface (`src/core/host.ts`)

```ts
interface Host {
  readonly name: string;
  readonly fs: {
    readText(path: string, options?: { fatal?: boolean }): string; // ENOENT; EILSEQ when fatal
    writeText(path: string, content: string, options?: { mode?: number }): void;
    mkdir(path: string, options?: { recursive?: boolean }): void;
    rm(path: string, options?: { force?: boolean; recursive?: boolean }): void;
    lstat(path: string): { isFile: boolean; isDirectory: boolean; isSymbolicLink: boolean };
    realpath(path: string): string;
    readdir(path: string): { name: string; isFile: boolean; isDirectory: boolean }[]; // symlinks: both false
  };
  readonly path: { sep; resolve; join; normalize; dirname; basename; relative; isAbsolute };
  readonly os: {
    platform: string;
    availableParallelism(): number;
    totalmem(): number;
    homedir(): string | undefined;
    tmpdir(): string;
  };
  cwd(): string;
  env(): Readonly<Record<string, string | undefined>>;
  spawn(options: {
    file: string; args: readonly string[]; cwd: string;
    env: Readonly<Record<string, string>>;          // complete; nothing inherited
    stdin?: string;
    stderr?: "pipe" | "merge" | "ignore";
    signal?: AbortSignal;                           // abort kills the process tree
    onOutput?: (stream: "stdout" | "stderr", chunk: string) => void;
  }): { pid: number | undefined; result: Promise<{ code: number | null; signal: string | null; stdout: string; stderr: string }> };
  createAsyncContext<T>(): { get(): T | undefined; run<R>(value: T, fn: () => R): R };
  now(): number;
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(timer: unknown): void;
  randomUUID(): string;
}
```

All filesystem calls are synchronous on purpose (selection, containment, staging, and the
commit already are; one JS thread is the only writer; synchronous Rust host functions are the
simplest thing a QuickJS host exposes). Errors carry a POSIX `code` string; callers depend on
`ENOENT` and `EILSEQ`. `spawn` never throws; start failures reject `result`.

Installation: `setHost(host)`, or `installDefaultHost(host)` (only when none is installed), or
publish the object as `globalThis.__codemodOrchestrationHost` (`HOST_GLOBAL`) before the bundle
is evaluated. Every portable function that needs the platform takes an optional `host` and
defaults to `getHost()`; `getHost()` throws `HostNotInstalledError` when nothing is installed.
Defining runnables and commands needs no host.

## Async context

Commands find their run through an `AsyncContextVariable` (the TC39 `AsyncContext.Variable`
shape: `get()`, `run(value, fn)`), created from the host the first time a run binds a runtime.
Static composition already passes runtimes explicitly (`runStage(runtime, ...)`); only commands
and composition nodes awaited by author code in `dynamic()` bodies (and the root body) read the
variable.

- **Node** (the Node host: the test harness and Node consumers): `AsyncLocalStorage`.
- **QuickJS:** `host/quickjs/async-context.ts`. Promise hooks do not work on
  the vendored quickjs-ng (`rquickjs-sys` 0.11): `JS_SetPromiseHook` fires `BEFORE`/`AFTER`
  only around thenable-resolution jobs, not around ordinary reaction jobs
  (`promise_reaction_job` calls no hook), and `await` registers its continuation without a
  derived promise. Instead the quickjs bundle is built with async functions, async generators,
  and `for await` lowered to generators (oxc's async-to-generator transforms in
  `crates/orchestration-build/src/bundle.rs`), so every continuation in the
  bundle goes through `Promise.prototype.then`, which the entry patches once to capture the
  current frame at registration and restore it around the callback: V8's
  `AsyncLocalStorage` rule. `tests/quickjs-async-context.test.ts` checks it against
  `AsyncLocalStorage` (nesting, `Promise.all`, `finally`, a promise resolved outside the
  run, async generators); `crates/cli/tests/typescript_workflow.rs`
  (`commands_issued_after_awaits_in_parallel_bodies_are_concurrent`) checks that `dynamic()`
  inside `parallel()` marks every command `concurrent`, as `AsyncLocalStorage` does. Transform artifacts
  are not lowered; they never read the run context.
- Rejected: explicit context passing to authors (breaks the authoring API) and QuickJS promise
  hooks (incomplete on the vendored engine, see above).

## Runtime registry contract

Portable (`src/runtime/capabilities.ts`), enforced by `run()` itself when
`RunSettings.runtime` is set, so it holds wherever the control plane runs:

```ts
interface RuntimeCapabilities {
  operations: Record<"shell" | "jssg" | "agent" | "assessment", boolean>;
  semanticAnalysis: boolean;
}
interface RuntimeDescriptor { name: string; capabilities: RuntimeCapabilities }
// QUICKJS_RUNTIME: every operation kind, semantic analysis.
// RUNTIME_NAMES = ["quickjs"]; DEFAULT_RUNTIME = "quickjs".
```

The built-in runtime supports everything, so the contract only refuses for a registered
runtime that declares less (`tests/capabilities.test.ts` uses such a `limited` runtime):

- Statically, before anything runs, every operation node of the static IR is checked
  (`assertRuntimeSupports`): `UnsupportedInRuntimeError` such as
  `operation kind 'agent' is unsupported in runtime 'limited' (command 'fix')`. Nothing is
  recorded.
- At the execution boundary (`CapabilityExecutor`, outside the scheduler, so no permit is
  taken), operations issued inside `dynamic()` that the runtime cannot execute complete as
  `failed` with `details: { phase: "config", runtime, unsupported, repositoryMayBeModified: false }`;
  the awaited command rejects with `OperationError`.

There is no Node-side provider or registry any more: `codemod workflow run -w` is the one
launcher, and it runs `quickjs`. It checks that the workflow and target exist and that
`--input` is JSON, builds or reuses the cached bundle, and runs it in-process (`src/host/runtimes/`,
with its `RuntimeProvider` contract and `runWorkflow`, was removed with the Node launcher).

## JSSG batches in-process

The `quickjs` runtime runs JSSG batches in-process through one portable loop,
`src/execution/jssg-batch.ts` (`createJssgBatchDispatch({ engine })`), and every rule of a
batch is Rust, in one place: `crates/jssg-batch` (`BatchPlan`), used by the QuickJS host
(`crates/orchestration-quickjs-host`). Nothing re-implements them in TypeScript.

| Step | Rust (`jssg-batch`) | `quickjs` (host primitives) |
| --- | --- | --- |
| plan: hash check, canonical root, every path a safe relative path, language, selector, params, semantic provider | `BatchPlan::for_paths(.., Containment::PerFile)` | `jssgBegin` |
| workspace semantic analysis only: index every file before the first transform | `BatchPlan::index` | `jssgIndex`: on the JS thread |
| contain each path in the root (`stat` + `realpath`), then parse a chunk (64 files); `null` where the selector matches nothing | `BatchPlan::parse` | `jssgParse`: on the JS thread |
| call the transform, staging `write()`/`jssgTransform()` | (sandbox result rules) | `jssgCall` / `jssgCallEnd` |
| result -> `FileOutcome` (string/null/`{ content, output }`, rename, `/` paths after containment) | `classify_transform_result` + `BatchPlan::finish` | `jssgFinish` |
| batches of 64 files or more: parse, transform, and convert whole chunks on the transform pool | `BatchPlan` per worker | `jssgTransformRange` |

A path is contained right before its file is parsed (`Containment::PerFile`), so the
filesystem calls overlap the transforms of the previous chunk; a path that escapes still fails
the command before that file reaches a transform, and nothing is written unless every file
passed. (`Containment::Upfront`, every path while planning, remains for a runner that has
every file's content up front; the removed bridge was the one user.)

Semantic analysis (`semanticAnalysis: "file" | "workspace" | { mode, root }`): `BatchPlan`
builds the batch's one provider (`semantic_factory::LazySemanticProvider`, file or workspace
scope) and attaches it to every parsed root, so `definition()` / `references()` answer the
same way as in the YAML engine. In workspace mode (`JssgBatch.indexesWorkspace`) the loop
first feeds every batch file to `index` (QuickJS `jssgIndex`), including files the selector
skips, before the first transform.

The loop owns only chunking, the workspace index pass, cancellation between files, evaluating
each artifact once per run (cached by hash, lazily: only when a file reaches a transform), and
turning failures into a `failed` completion. The bindings only read the transform's
JavaScript return value into plain data (`ReturnedContent` + output JSON).
`crates/cli/tests/jssg_parity.rs` runs edge cases (results of every type, structured outputs,
throws, per-file skips, the transform pool, the selector skip, multiple and staged edits,
renames and writes escaping the root, also through a symlink) through `quickjs` and pins the
outputs, trees, and errors. The YAML engine is the external reference for the encoding case
(above) and for the benchmark transforms (`benchmarks/runtimes/verify.sh`).

By design, module-level state in a transform persists across the files and commands of one
run (per worker on the transform pool; unsupported), and a transform that never returns
cannot be interrupted (the signal is checked between files).

## What a QuickJS host must provide

(The original requirements list; `crates/orchestration-quickjs-host` plus
`src/host/quickjs/*` now implement it. Differences: the `Host` is built by the bundle entry
from Rust primitives, and the async context uses then-propagation, not promise hooks.)

1. A `Host` object (above), installed via `HOST_GLOBAL` before the bundle evaluates or by the
   bundle entry calling `setHost`:
   - `fs`: synchronous UTF-8 read (lossy and fatal/`EILSEQ`), write with mode, mkdir -p, rm
     (force/recursive), lstat, realpath, readdir with symlink-free typing; errors with `code`.
   - `path`: may be `createPosixPath(() => cwd)` from the bundle itself (Unix).
   - `os`: platform, CPU count, total memory, home, tmpdir. `cwd()`, `env()`.
   - `spawn`: `sh -c` capable, full explicit env, stdin, stdout+stderr merged into one pipe
     (`stderr: "merge"`), abort kills the process tree (spawn in a new group; descendants that
     left it are found by one `ps` snapshot), resolves with code/signal/output. This is what
     `executeShell` needs for DirectRunner parity and `executeAgent` needs for the runner.
   - `createAsyncContext`: then-propagating (see "Async context"; the requirement first
     proposed promise hooks).
   - `now`, `setTimeout`/`clearTimeout` (the external agent timeout uses them), `randomUUID`.
2. Globals the portable closure uses: `Promise`, `Map`/`Set`/`WeakMap`, `JSON`, `Symbol`,
   `globalThis`, `queueMicrotask`-equivalent job queue (QuickJS has it), and
   `AbortController`/`AbortSignal` with `aborted`, `reason`, `addEventListener`/
   `removeEventListener` (QuickJS has none; provide a small polyfill). No `process`,
   `Buffer`, `TextDecoder`, timers-as-globals, or `require` are needed
   (`tests/portable.test.ts` enforces this for everything reachable from `portable.ts`).
3. The bundle: built by `crates/orchestration-build` (rolldown) from an entry that imports
   `@codemod.com/orchestration/portable` (alias `@codemod.com/orchestration` to it), the
   workflow module, and the transform artifacts; the only bundled npm dependency is
   `ignore` (pure JS, CommonJS). `languages.json` is a JSON import.
4. An entry that calls `run(workflow, { executor, runtime: QUICKJS_RUNTIME, input })`, where
   `executor` routes `jssg` to `executeJssgCommand({ dispatch })` (dispatch evaluates each
   artifact module once per run, cached by hash, and runs transforms serially per file against
   the snapshot), `shell` to `executeShell`, `assessment` to `executeAssessmentOperation`
   (with a `fetch` over the host's `httpRequest`), and `agent` to `executeAgent` (the runner
   from `CODEMOD_AGENT_BIN`). JSSG transactional semantics (snapshot,
   cross-file conflicts, nothing written before every transform succeeded, containment) stay in
   `executeJssgCommand` and need nothing extra from the host.

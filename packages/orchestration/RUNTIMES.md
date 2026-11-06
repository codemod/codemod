# Runtimes and the host seam

> Status: prototype. `bridge` is the default and only implemented runtime;
> `node` and `quickjs` are registered placeholders that fail with
> `runtime '<name>' not available yet`.

```text
codemod-workflow [run] <workflow.ts> [--runtime <bridge|node|quickjs>] ...
runWorkflow(path, { runtime })                      // programmatic, Node
run(executable, { executor, runtime: NODE_RUNTIME }) // any host, incl. QuickJS
createHarness({ runtime: "node" })                   // tests
```

| Runtime | Control plane | `jssg` | `shell` | `agent` / `assessment` | Semantic analysis |
| --- | --- | --- | --- | --- | --- |
| `bridge` (default) | Node | one bridge process per command | one bridge process | bridge / Node host | yes |
| `node` | Node | in-process over a napi addon | `child_process` via `Host.spawn` | unsupported | unsupported |
| `quickjs` | QuickJS (Rust binary) | in-process in the same context | Rust host spawn | unsupported | unsupported |

## Files

| Path | What |
| --- | --- |
| `src/core/host.ts` | `Host` interface, `setHost`/`getHost`/`installDefaultHost`, `HOST_GLOBAL` |
| `src/core/posix.ts` | dependency-free port of `node:path.posix` (`createPosixPath`) |
| `src/core/async-context.ts` | `createFallbackAsyncContext` (stopgap, see below) |
| `src/host/node-host.ts` | Node `Host` (`NODE_HOST`, `createNodeHost`); importing it installs it |
| `src/execution/operation-executor.ts` | `OperationExecutor` (moved out of `executor.ts`) |
| `src/execution/jssg.ts` | portable `executeJssgCommand({ ..., dispatch, host })` |
| `src/execution/bridge.ts` | `executeJssg` = `executeJssgCommand` with `spawnBridge` as `dispatch` |
| `src/execution/shell.ts` | portable `executeShell` with DirectRunner semantics |
| `src/runtime/capabilities.ts` | runtime descriptors, `CapabilityExecutor`, `assertRuntimeSupports` |
| `src/host/runtimes/types.ts` | `RuntimeProvider` contract |
| `src/host/runtimes/{bridge,node,quickjs}.ts` | one provider per runtime |
| `src/host/runtimes/registry.ts` | `RuntimeRegistry`, `defaultRuntimes`, `runWorkflow`, `prepareLaunch` |
| `src/portable.ts` | `@codemod.com/orchestration/portable`: the Node-free API |
| `tests/portable.test.ts` | static import scan of everything reachable from `portable.ts` |

## Inventory: what used Node, and where it went

Needed to run a `jssg` + `shell` workflow, now behind `Host`:

| Was | Now |
| --- | --- |
| `authoring/context.ts`: `AsyncLocalStorage` | `Host.createAsyncContext()` (created lazily, once per realm) |
| `authoring/target.ts`: `node:path.posix.normalize` | `core/posix.ts` |
| `core/paths.ts`: `lstatSync`, `realpathSync.native`, `node:path` | `Host.fs.lstat/realpath`, `Host.path` |
| `execution/files.ts`: `readFileSync`, `readdirSync`, `homedir`, `node:path`, `process.env`, `languages.json` via `fs` | `Host.fs.readText/readdir`, `Host.os.homedir`, `Host.path`, `Host.env()`, JSON import attribute |
| `execution/jssg.ts`: `node:fs` read/write/mkdir/rm/realpath, `node:path`, `TextDecoder`, direct `spawnBridge` | `Host.fs`, `Host.path`, `readText(..., { fatal })`, injected `dispatch` |
| `execution/scheduler.ts`: `node:os` `availableParallelism`/`totalmem`, `process.env` | `Host.os`, `Host.env()` (`nodeHost` moved to `host/node-host.ts`) |
| (new) `shell` in-process | `execution/shell.ts` over `Host.spawn`, `Host.fs`, `Host.os.tmpdir`, `Host.randomUUID` |

`runtime/run.ts`, `runtime/gate.ts`, `core/history.ts`, and `authoring/composition.ts` used
no Node API directly; they now reach none transitively.

Node-only, stays Node-only (`NODE_ONLY` in `tests/portable.test.ts`):

- `bundle/build.ts`: esbuild + TypeScript + `node:module` hooks; runs ahead of time.
- `execution/bridge.ts`, `exchange.ts`, `process-tree.ts`, `executor.ts`: the bridge runtime
  (exchange files with `O_EXCL`/`O_NOFOLLOW`, process groups, tree kill).
- `execution/agent-env.ts`, `execution/assessment.ts`: agents and the TypeSafe client; refused
  outside `bridge` by the capability contract.
- `host/**`: CLI argument parsing, dashboard server, harness, runtime providers, Node host.

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

- **Node:** `AsyncLocalStorage`.
- **QuickJS (recommended):** the vendored engine is quickjs-ng (`rquickjs-sys` 0.11 exposes
  `JS_SetPromiseHook` with `INIT`/`BEFORE`/`AFTER`/`RESOLVE`, the same primitive V8 uses to back
  `AsyncLocalStorage`). The Rust host keeps one "current context" slot per variable: on `INIT`
  store the current slot on the new promise (`parent_promise`'s for derived promises); on
  `BEFORE` save the current slot and switch to the promise's; on `AFTER` restore. `run(value,
  fn)` sets the slot for the synchronous extent of `fn` and restores it afterwards. This keeps
  `concurrent` flags and `dynamicId`s identical to Node, so history is byte-identical across
  runtimes.
- **QuickJS (stopgap):** `createFallbackAsyncContext()` in `core/async-context.ts`. Correct for
  sequential workflows and all static composition; after an `await`, a nested `dynamic()` body
  falls back to the root runtime, so a `dynamic()` inside `parallel()` loses `concurrent: true`
  (history differs from Node; replay may report `reordered`) and nested dynamic commands lose
  `dynamicId` in events. Covered by `tests/portable.test.ts`.
- Rejected: explicit context passing to authors (breaks the authoring API) and patching
  `Promise.prototype.then` (native `await` bypasses it unless async functions are lowered to
  generators at build time; possible later, but promise hooks are simpler and exact).

## Runtime registry contract

Portable (`src/runtime/capabilities.ts`), enforced by `run()` itself when
`RunSettings.runtime` is set, so it holds wherever the control plane runs:

```ts
interface RuntimeCapabilities {
  operations: Record<"shell" | "jssg" | "agent" | "assessment", boolean>;
  semanticAnalysis: boolean;
}
interface RuntimeDescriptor { name: string; capabilities: RuntimeCapabilities }
// BRIDGE_RUNTIME: everything. NODE_RUNTIME, QUICKJS_RUNTIME: shell + jssg, no semantics.
```

- Statically, before anything runs, every operation node of the static IR is checked
  (`assertRuntimeSupports`): `UnsupportedInRuntimeError` such as
  `operation kind 'agent' is unsupported in runtime 'node' (command 'fix')`. Nothing is
  recorded.
- At the execution boundary (`CapabilityExecutor`, outside the scheduler, so no permit is
  taken), operations issued inside `dynamic()` that the runtime cannot execute complete as
  `failed` with `details: { phase: "config", runtime, unsupported, repositoryMayBeModified: false }`;
  the awaited command rejects with `OperationError`. JSSG `semanticAnalysis` is refused the same
  way (`JSSG semantic analysis is unsupported in runtime 'node'`).

Node-side provider (`src/host/runtimes/types.ts`), one file per runtime, listed in
`registry.ts`:

```ts
interface RuntimeProvider extends RuntimeDescriptor {
  description: string;
  availability(settings: RuntimeSettings): { available: true } | { available: false; reason: string };
  createExecutor?(context: { cwd; artifacts; events?; settings }): OperationExecutor; // in-process
  launch?(request: { workflow; target; input?; signal?; settings; notify? }): Promise<unknown>; // out-of-process
}
```

`runWorkflow`/the CLI: resolve the name (`unknown runtime 'x'; expected one of: ...`), check
the workflow and target exist, check `availability` (its `reason` is the whole error; the CLI
exits 1), then either `launch` the file or load it (`loadWorkflow`) and
`run(executable, { executor: createExecutor(...), runtime: provider })`. `--dashboard` needs
`createExecutor`.

To add `node`: replace `src/host/runtimes/node.ts` with a provider whose `availability` checks
for the addon and whose `createExecutor` returns an executor that routes `jssg` to
`executeJssgCommand({ ..., dispatch: inProcessBatch })` and `shell` to `executeShell`. To add
`quickjs`: replace `src/host/runtimes/quickjs.ts` with a `launch` that builds the bundle and
spawns the binary. No CLI change either way.

## What a QuickJS host must provide

1. A `Host` object (above), installed via `HOST_GLOBAL` before the bundle evaluates or by the
   bundle entry calling `setHost`:
   - `fs`: synchronous UTF-8 read (lossy and fatal/`EILSEQ`), write with mode, mkdir -p, rm
     (force/recursive), lstat, realpath, readdir with symlink-free typing; errors with `code`.
   - `path`: may be `createPosixPath(() => cwd)` from the bundle itself (Unix).
   - `os`: platform, CPU count, total memory, home, tmpdir. `cwd()`, `env()`.
   - `spawn`: `sh -c` capable, full explicit env, stdin, stdout+stderr merged into one pipe
     (`stderr: "merge"`), abort kills the process group (spawn in a new group), resolves with
     code/signal/output. This is what `executeShell` needs for DirectRunner parity.
   - `createAsyncContext`: promise-hook backed (see above), or the fallback.
   - `now`, `setTimeout`/`clearTimeout` (the control plane itself does not use timers today),
     `randomUUID`.
2. Globals the portable closure uses: `Promise`, `Map`/`Set`/`WeakMap`, `JSON`, `Symbol`,
   `globalThis`, `queueMicrotask`-equivalent job queue (QuickJS has it), and
   `AbortController`/`AbortSignal` with `aborted`, `reason`, `addEventListener`/
   `removeEventListener` (QuickJS has none; provide a small polyfill). No `process`,
   `Buffer`, `TextDecoder`, timers-as-globals, or `require` are needed
   (`tests/portable.test.ts` enforces this for everything reachable from `portable.ts`).
3. The bundle: built by Node with esbuild from an entry that imports
   `@codemod.com/orchestration/portable` (alias `@codemod.com/orchestration` to it), the
   workflow module, and the transform artifacts; the only bundled npm dependency is `ignore`
   (pure JS, CommonJS). `languages.json` is a JSON import.
4. An entry that calls `run(workflow, { executor, runtime: QUICKJS_RUNTIME, input })`, where
   `executor` routes `jssg` to `executeJssgCommand({ dispatch })` (dispatch evaluates each
   artifact module once per run, cached by hash, and runs transforms serially per file against
   the snapshot) and `shell` to `executeShell`. JSSG transactional semantics (snapshot,
   cross-file conflicts, nothing written before every transform succeeded, containment) stay in
   `executeJssgCommand` and need nothing extra from the host.

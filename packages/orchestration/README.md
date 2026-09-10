# @codemod.com/orchestration (prototype)

TypeScript-first prototype of the Codemod orchestration runtime. Workflows are
plain async TypeScript; calling a runnable creates a command, awaiting it inside
a workflow issues it, and every issued command is recorded in an append-only
history and replayed from that history on later runs. A small Rust execution
bridge runs `exec` through `butterflow_runners::DirectRunner` and local JSSG
operations through the existing QuickJS sandbox (see `RUST_BRIDGE.md`). The evidence,
problem statement, proposal, boundaries, and migration path are summarized in
`DESIGN.md`.

## Layout

```
packages/orchestration/
  DESIGN.md         problem statement, proposal, scope, and migration path
  src/protocol.ts   versioned JSON OperationRequest / OperationCompletion
  src/runnable.ts   callable exec / jssg / ai descriptors (typed via Standard Schema)
  src/command.ts    Command: one invocation as data, awaitable inside a workflow
  src/context.ts    the active workflow runtime (AsyncLocalStorage in the Node prototype)
  src/target.ts     validation and normalization of a JSSG invocation Target
  src/paths.ts      safe-relative-path rules shared by targets, scripts, and the wire
  src/cli.ts        experimental local runner behind bin/codemod-workflow.mjs
  src/plan.ts       plan(...) and parallel(...) groups + JSON IR
  src/workflow.ts   workflow(async () => ...) and run(executable, options)
  src/history.ts    HistoryStore seam + MemoryHistoryStore
  src/gate.ts       CommandGate seam + ReplayGate (replay matching, errors)
  src/executor.ts   OperationExecutor seam + BridgeExecutor (spawns Rust bridge)
  src/events.ts     EventSink seam
  src/harness.ts    test harness with scripted completions
  fixtures/protocol shared JSON fixtures checked by TS and Rust tests
  tests/            scenario tests (fast, no Rust) + bridge.e2e.test.ts
  bin/              codemod-workflow.mjs and its node_modules type-stripping hook
crates/execution-bridge/  serde structs, Runner call, and the JSSG adapter (no orchestration logic)
  src/main.rs             `butterflow-execution-bridge <request.json> <response.json>`
```

## Authoring

```ts
import { exec, guard, jssg, plan, parallel, workflow } from "@codemod.com/orchestration";

const Project = guard("Project", (v: unknown): v is { needsMigration: boolean } => /* ... */);
const Summaries = guard("Summaries", (v: unknown): v is { file: string }[] => Array.isArray(v));

const inspect = exec({ name: "inspect", command: "node inspect.js", output: Project });
const migrate = jssg({
  name: "migrate",
  script: "scripts/migrate.ts", // relative to the workflow file's directory
  language: "tsx",
  include: ["**/*.{ts,tsx}"],
  semanticAnalysis: "workspace",
  input: Project,
  output: Summaries,
});

export default workflow(async () => {
  const project = await inspect();
  if (project.needsMigration) await migrate({ input: project });
  return project;
});

// fixed plan
export default plan(rename, updateImports, format);
// explicit assertion that these operations have no ordering dependency
export default plan(parallel(countTodos, countFixmes), format);

// JSSG invocations carry a file target; exec and ai never do
const web = { root: "apps/web", include: ["src/**"], exclude: ["**/generated/**"] };
export default plan(rename({ target: web }), updateImports({ target: web }), format);
export default parallel(transformA({ target: web }), transformB({ target: web }));
export default workflow(async () => {
  const project = await inspect();
  const summaries = await parallel(
    project.packages.map((pkg) =>
      migrate({ input: project, target: { root: pkg.path }, id: `migrate:${pkg.name}` }),
    ),
  );
  return summaries.length;
});
```

- Calling a runnable creates a `Command`: `inspect()`, `lint({ id })`,
  `migrate({ input, target, id })`. Creating one does nothing. Awaiting it
  inside a workflow body issues it (replay or execute) and returns its typed
  output; awaiting it anywhere else rejects with `NoActiveWorkflowError`. A
  command issues at most once per run however often it is awaited.
- `exec` and `ai` invocations accept `id` and `input` only. `jssg` invocations
  also accept `target`. A `target` on `exec` or `ai` throws
  `TargetValidationError` when the command is created; any other unknown field
  throws `InvocationError`.
- `exec` output: with an `output` schema, the runner's returned text is parsed
  as JSON and validated; without one the output is `{ stdout }`. The field name
  is provisional: the existing `DirectRunner` combines stdout and stderr on
  Unix but returns stdout alone on other platforms.
- Repeated invocations of the same runnable need an explicit id, for example
  `lint({ id: "lint:" + i })`. A repeated invocation without an id throws
  `DuplicateCommandIdError`. Unique invocations use the runnable name as their id.
- Non-success completions (`failed`, `cancelled`, `unknown`) reject the awaited
  command with `OperationError`; catch it to branch.
- Workflow return values and operation outputs are plain JSON.
- `plan(...)` and `parallel(...)` are data too. A bare runnable in either stands
  for its default command. Both are awaitable inside a workflow; `run(plan)`
  runs a plan on its own. `parallel` takes members spread (a fixed group) or as
  one array (a group built inside a workflow, such as one command per
  discovered package). Members start in declaration order and outputs come
  back in that order.
- `parallel(...)` is an author assertion. The prototype starts each member as a
  whole concurrent operation; it does not implement per-file locking. Do not
  place dependent mutations or opaque commands that may conflict in one group.
  `DESIGN.md` describes the future JSSG file scheduler.
- A JSSG `target` is `{ root?, include?, exclude? }`. It is validated when the
  command is created (relative `root` without `..`, non-empty pattern lists, no
  unknown fields, not empty), recorded in history as command content, and sent
  on the wire as `operation.target`. Changing it under the same id replays as
  `changed`. There is no generic `target()` wrapper and no `shard()`/`scope()`
  helper; sharding and worker counts are scheduler behavior.
- A JSSG definition supplies `script`, `language`, optional intrinsic
  `include`/`exclude`, and optional `semanticAnalysis`: `"file"`,
  `"workspace"`, or `{ mode: "file" | "workspace", root? }` where `root` is a
  safe relative path beneath the target root and is only valid with
  `workspace`. Without `include`, the definition applies to the language's
  file extensions, exactly as a YAML `js-ast-grep` step without `include`.
- `script` is a safe relative path (no leading `/`, no drive letter, no `..`
  segment). That relative path is the command identity recorded in history, so
  a history replays on another checkout. The executor resolves it against its
  script root: the workflow file's directory for `codemod-workflow`, or
  `BridgeOptions.scriptRoot` for `BridgeExecutor`. The root travels as
  `request.context.scriptRoot`, which is never part of a recorded command.
- The script's default export may return the existing `string | null`
  (`Codemod<T>` in `@codemod.com/jssg-types`) or `{ content?, output }`
  (`StructuredCodemod<T, O>`). The bridge applies `content` like any JSSG
  result and returns the present `output` values as an array in sorted file
  order; a `string | null` transform yields `[]`. Only the top-level transform
  may be structured: `jssgTransform` accepts a plain `Codemod` and the sandbox
  rejects a structured result from a secondary transform with an error rather
  than discarding it. Invocation input reaches the transform as
  `options.params.input` (typed by `StructuredTransformOptions`); the shipped
  selector engine passes no params, so `getSelector` sees `{}` as before.
- The JSSG bridge intersects definition `include`/`exclude` patterns with the
  invocation target, sorts matching paths, and executes each file's
  read-transform-write cycle serially. Files are enumerated with the same
  walker settings as the workflow engine: hidden files are visited, `.gitignore`
  and `.ignore` apply even outside a git repository, symlinks are not followed.
  Files that vanish before they are read or that are not valid UTF-8 are
  skipped, as the engine does. A renamed file's original is removed only after
  every enumerated file has run. `exec` runs in the executor's `cwd`.
  The local JSSG profile grants no process, network, unrestricted filesystem,
  LLM, or mutable workflow-state capabilities; the curated `fs` module is
  limited to the target root.
- JSSG completion status: configuration, script resolution, glob, selector,
  semantic-index, and transform errors that happen before the bridge has written
  any file are `failed` (nothing changed). An error after the first write is
  `unknown` because earlier files may already be modified. A transform that
  writes through the curated `fs` module before failing is not tracked.
- A body that returns while a command it created is unissued, or while an
  issued command is still running, is not finalized: the runtime waits for
  running work and then throws.

## Running

For the trusted local prototype, run a TypeScript workflow with the
experimental `codemod-workflow` bin (Node 24):

```sh
cargo build -p butterflow-execution-bridge
pnpm --filter @codemod.com/orchestration workflow ./path/to/workflow.ts --target ./repository
# or, once the package is installed in another project:
npx codemod-workflow ./workflow.ts --target ./repository --bridge /path/to/butterflow-execution-bridge
```

```text
codemod-workflow <workflow.ts> [--target <directory>] [--script-root <directory>] [--bridge <binary>]
```

- `--target` (default: current directory) is where `exec` runs and the root
  JSSG targets are resolved beneath.
- `--script-root` (default: the workflow file's directory) is what a JSSG
  definition's relative `script` resolves against.
- `--bridge` or `CODEMOD_BRIDGE_BIN` (default: the monorepo's
  `target/debug/butterflow-execution-bridge`) locates the bridge binary; the
  command fails early when it is missing.

The command prints the workflow's final value as JSON. The bin re-spawns Node
with `--experimental-transform-types` and a `node:module` hook that strips
types from `.ts` files under `node_modules`, so it works both from this
checkout and from a workspace or package installation of
`@codemod.com/orchestration` (the e2e suite installs a copy under a consumer's
`node_modules` to prove it). The workflow module and the sources it imports are
expected to be ESM. No build step or `dist/` exists.

Remaining work before Solid Migration Assistant can move onto this package:

- add the package to that repository and author its workflow, its JSSG scripts
  as relative `script` paths, and its structured outputs (`StructuredCodemod`);
- ship the bridge binary with the package or as a `codemod` subcommand; today it
  must be built from this monorepo and pointed at with `--bridge`;
- an AI executor adapter, `pipe()`, approvals, and durable history remain
  unimplemented, so any Solid step that needs them stays in YAML;
- restricted QuickJS execution of the workflow body and registry loading are
  still required before TypeScript workflows become an untrusted registry format.

The programmatic API is:

```ts
import { BridgeExecutor, MemoryHistoryStore, run } from "@codemod.com/orchestration";

const executor = new BridgeExecutor({
  bin: "target/debug/butterflow-execution-bridge",
  cwd: repoDir, // exec cwd and JSSG target root
  scriptRoot: workflowDir, // what relative jssg `script` paths resolve against
});
const history = new MemoryHistoryStore();
const first = await run(workflowModule, { executor, history });
const again = await run(workflowModule, { executor, history: MemoryHistoryStore.fromJSON(history.serialize()) });
// again.replayed === true, nothing was executed
```

## Replay semantics

History is `{ protocolVersion, events[] }` with three event types:
`scheduled` (command), `completed` (terminal completion), `finalized`
(workflow output). Commands are matched by id and by the canonical JSON of the
whole command record. `ReplayGate` raises `NondeterminismError` with a `kind`:

| kind        | meaning                                                            |
| ----------- | ------------------------------------------------------------------ |
| `changed`   | same id with different content, or a recorded command was replaced |
| `reordered` | a recorded command was issued after later recorded commands         |
| `removed`   | recorded commands were skipped (middle) or never issued (trailing)  |
| `added`     | a new command was issued after the workflow had finalized           |
| `output`    | the final output differs from the recorded one                      |

A command that was scheduled but never completed replays as `unknown`.
An unfinalized history replays its recorded commands and then executes new
ones, which is how a crashed run resumes.

## How a command finds its workflow

The body receives no context argument. `run()` binds the run's runtime to the
body with Node's `AsyncLocalStorage`, so `await inspect()` anywhere in the
body's async continuations issues to that run, two concurrent runs never see
each other's runtime, and nothing is stored on a process-global. A command
awaited outside any run rejects instead of running. In production the same
binding belongs to the host: a restricted QuickJS instance would expose the
runtime to the workflow bundle it executes, with no ambient storage at all.

## Determinism warning

Normal Node execution is NOT a secure deterministic sandbox. Workflow code only
needs the runnables it imports, but nothing prevents it from reading `Date`,
`Math.random`, `fs`, or `process`. Determinism is validated after the fact by
replay comparison; if a workflow uses such inputs the replay will fail with
`NondeterminismError`. QuickJS sandboxing is out of scope for this prototype.

## Migration seams

`OperationExecutor`, `HistoryStore`, `CommandGate`, and `EventSink` are small
interfaces with JSON-only inputs and outputs. Each in-memory implementation can
move to Rust one at a time without changing workflow source. `exec` and local
`jssg` have bridge adapters; `ai` remains protocol-only (the bridge answers
`failed` with "no executor adapter") and the harness can still script any
operation in unit tests. `OperationRequest.context` is the executor's own
input (currently `scriptRoot`); it is validated strictly on both sides and is
never written to history.

## Commands

```
pnpm install
pnpm --filter @codemod.com/orchestration test          # fast TS tests, no Rust
pnpm --filter @codemod.com/orchestration typecheck
pnpm --filter @codemod.com/orchestration test:e2e     # builds only the bridge crate, then cross-language test
cargo test -p butterflow-execution-bridge              # Rust protocol, runner, JSSG adapter, and binary tests
```

The full `codemod` CLI is never built or used by this package.

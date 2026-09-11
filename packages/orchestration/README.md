# @codemod.com/orchestration (prototype)

TypeScript-first prototype of the Codemod orchestration runtime. Workflows are
plain async TypeScript; calling a runnable creates a command, awaiting it inside
a workflow issues it, and every issued command is recorded in an append-only
history and replayed from that history on later runs. `exec` runs through a
small Rust bridge over `butterflow_runners::DirectRunner`. JSSG transforms are
written inline next to the workflow, split off into standalone bundles at
build time, and orchestrated here in TypeScript (file selection, ordering,
conflict checks, transactional commit, output aggregation, failure
classification) around one Rust bridge process per command that owns the
QuickJS sandbox and semantic providers and transforms the whole batch (see
`RUST_BRIDGE.md`). The evidence, problem statement, proposal, boundaries, and
migration path are summarized in `DESIGN.md`.

## Layout

```
packages/orchestration/
  DESIGN.md              problem statement, proposal, scope, and migration path
  RUST_BRIDGE.md         the Rust boundary: protocol, batch, security model, transactions
  src/protocol.ts        versioned JSON OperationRequest / OperationCompletion (v5)
  src/build.ts           build step: extract inline transforms, bundle, rewrite, loadWorkflow
  src/transform.ts       author-facing transform/selector types per language (type-only)
  src/bridge.ts          spawnBridge: one bridge process per request over exchange files
  src/jssg.ts            executeJssg: artifact, select, batch, validate, stage, commit, classify
  src/files.ts           file selection with the engine's walker semantics (npm `ignore`)
  src/languages.json     language -> extensions, pinned to the engine table by a cargo test
  src/paths.ts           safe-relative-path rules and root containment (realpath)
  src/runnable.ts        callable exec / jssg / ai descriptors (typed via Standard Schema)
  src/command.ts         Command: one invocation as data, awaitable inside a workflow
  src/context.ts         the active workflow runtime (AsyncLocalStorage in the Node prototype)
  src/target.ts          validation and normalization of a JSSG invocation Target
  src/cli.ts             experimental local runner behind bin/codemod-workflow.mjs
  src/plan.ts            plan(...) and parallel(...) groups + JSON IR
  src/workflow.ts        workflow(async () => ...) and run(executable, options)
  src/history.ts         HistoryStore seam + MemoryHistoryStore
  src/gate.ts            CommandGate seam + ReplayGate (replay matching, errors)
  src/executor.ts        OperationExecutor seam + BridgeExecutor (exec, jssg, ai refused)
  src/events.ts          EventSink seam (+ bridge.spawned)
  src/harness.ts         test harness with scripted completions
  fixtures/protocol      shared JSON fixtures checked by TS and Rust tests
  fixtures/walker        file-walker contract checked by TS and Rust (engine walker) tests
  tests/                 unit tests (fast, no Rust; fake bridge) + bridge.e2e.test.ts
  bin/                   codemod-workflow.mjs and its node_modules type-stripping hook
crates/execution-bridge/ protocol structs, exec through DirectRunner, one JSSG batch
  src/main.rs            `butterflow-execution-bridge <request.json> <response.json>`
```

## Authoring

```ts
import { exec, guard, jssg, plan, parallel, workflow } from "@codemod.com/orchestration";
import { rewriteSignal } from "./helpers.ts"; // bundled into the transform

const Project = guard("Project", (v: unknown): v is { needsMigration: boolean } => /* ... */);
const Summaries = guard("Summaries", (v: unknown): v is { file: string }[] => Array.isArray(v));

const inspect = exec({ name: "inspect", command: "node inspect.js", output: Project });
const migrate = jssg({
  name: "migrate-signals",
  language: "tsx",
  include: ["src/**/*.tsx"],
  semanticAnalysis: "workspace",
  selector: { rule: { pattern: "createSignal($VALUE)" } }, // optional static prefilter
  input: Project,
  output: Summaries,
  transform(root, options) {
    // root: SgRoot<TSX>, options.params.input: Project
    const edits = root.root().findAll({ rule: { pattern: "createSignal($VALUE)" } }).map(rewriteSignal);
    return { content: root.root().commitEdits(edits), output: { file: root.relativeFilename() } };
  },
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
  command with `OperationError`; catch it to branch. `error.details` carries
  structured data (for JSSG: the phase, and for commits the applied and
  remaining paths).
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
- A JSSG definition supplies `name`, `language`, the inline `transform`,
  optional intrinsic `include`/`exclude`, an optional static `selector`, and
  optional `semanticAnalysis`: `"file"`, `"workspace"`, or
  `{ mode: "file" | "workspace", root? }` where `root` is a safe relative path
  beneath the target root and is only valid with `workspace`. Without
  `include`, the definition applies to the language's file extensions, exactly
  as a YAML `js-ast-grep` step without `include`; the list is
  `src/languages.json`, which `cargo test -p butterflow-execution-bridge`
  checks against the engine's table so it cannot drift silently. Languages
  outside that table need an explicit `include`.

### The inline transform

`transform` is the one public transform function, with the same
`(root, options)` contract as a standalone codemod's default export
(https://docs.codemod.com/jssg/reference). `language` types it: `root` is
`SgRoot<TSX>` for `language: "tsx"`, and so on for every language with a
published type map in `@codemod.com/jssg-types`; other languages get the
untyped `TypesMap`. It may return the existing `string | null | undefined`
(`Codemod<T>`), or `{ content?, output }` (`StructuredCodemod<T, O>`) where
`content` is written like any JSSG result and the present `output` values
come back as an array in file order, typed as the element type of the
definition's `output` schema. An existing `Codemod<T>` value is assignable.
Invocation input reaches the transform as `options.params.input`. Only the
top-level transform may be structured: `jssgTransform` accepts a plain
`Codemod` and the sandbox rejects a structured result from a secondary
transform.

The workflow and its transforms are authored together but never run
together. A transform runs in the bridge's QuickJS sandbox, the workflow body
in Node; nothing is serialized with `Function.prototype.toString()` and no
closure crosses the boundary. Instead, a build step splits the module before
it runs (`src/build.ts`):

1. The module is parsed with the TypeScript compiler. Every
   `jssg({ ... })` call (an object literal with a string-literal `name` and a
   `transform` that is a method, a function or arrow expression, or an
   imported binding) is located by position.
2. For each transform, a virtual entry `export default <transform>` plus
   the import declarations it references is bundled with esbuild into one
   self-contained ES module: helpers imported from other modules (and their
   imports, including packages) are inlined; `codemod:*` modules and Node
   built-ins that the sandbox provides stay as imports; types are erased.
3. The artifact's identity is its `name` and the SHA-256 of the bundled
   source. The module is rewritten so `transform` is that `{ name, hash }`
   reference, and the rewritten module is what Node executes. Module comments
   in the bundle are relative paths, so the hash is the same on every
   checkout and changes whenever the transform or a bundled helper changes.

Supported subset, checked at build time with a source position rather than
failing in the sandbox:

- A transform may use its own parameters and locals, globals, and bindings
  the workflow module imports from other modules.
- It may not use anything else declared in the workflow module: a top-level
  `const`, `let`, `function`, or `class` is an unsupported lexical capture.
  Move such helpers into a module and import them; dynamic values enter
  through invocation input, never through capture.
- It may not use bindings imported from `@codemod.com/orchestration`; the
  orchestration runtime does not exist inside the sandbox.
- `name` must be a string literal and the argument an object literal.
  Generators are rejected. `jssg(options)` with a variable is not extracted.

A transform function that reaches `jssg()` at runtime means the module was
loaded without the build step; the call throws with that explanation.
`loadWorkflow(path)` (used by the CLI) imports a module through a
`node:module` load hook that applies the split to every module in its graph,
including definitions in imported files and packages under `node_modules`,
and returns the module namespace with the artifacts it collected.
`buildModule(source, file)` and `buildFile(file)` do the split without
importing anything. Artifacts are executor-side data (`BridgeExecutor({
artifacts })`); a JSSG command whose artifact the executor does not hold
fails in phase `artifact` before anything is spawned.

`esbuild` is the one dependency added for this: it is the bundler the
monorepo already resolves (through Vite), it bundles TypeScript and package
imports without configuration, and its synchronous API runs inside the
loader hook. `typescript`, already a development dependency, provides the
parser; it moved to `dependencies` so the installed package can build.

### The static selector

`selector` is optional ast-grep rule data, `{ rule, constraints?, utils? }`,
typed for the definition's language. The executor evaluates it natively,
before any transform sandbox starts, on every selected file; files without a
match are skipped and produce no edit and no output. It is an eligibility
prefilter and nothing else: the transform finds its nodes with the normal
`root.find` / `root.findAll` APIs, and `options.matches` is not populated.
Without a selector every selected file runs. Workspace semantic indexing
always covers the full selected set, so a matching transform can resolve
definitions and references in files the selector skipped. The selector is
part of the recorded command, like `include` and `exclude`.

This differs from the legacy `getSelector()` export, which Butterflow, the
`codemod jssg` commands, and `jssg list-applicable` continue to support
unchanged: that function is executed in QuickJS to obtain the rule, and its
matches are handed to the transform as `options.matches`. On this path the
selector is data, is never executed, and the artifact's exports other than
the default are ignored.

### How a JSSG command runs

1. The executor looks up the artifact the operation names by hash.
2. The target root is resolved beneath the executor's working directory and
   proven to stay there (symlinks out of the repository are rejected).
3. TypeScript selects the effective file set: files under the target root
   accepted by the definition's applicability (repository-relative
   `include`/`exclude`, defaulting to `**/*<ext>` for the language) and by the
   invocation target (target-root-relative `include`/`exclude`). The walker
   has the workflow engine's semantics, pinned by `fixtures/walker/cases.json`
   which the Rust engine walker must also satisfy: hidden files and `.git`
   contents are visited, `.ignore`, `.gitignore`, `.git/info/exclude`, and the
   global git excludes apply without requiring a git repository, ignore files
   in ancestor directories apply, symlinks are skipped and never followed,
   and include/exclude globs take precedence over every ignore file (so a
   language default or include glob whitelists a gitignored file, while a
   gitignored directory is never entered). Order is component-wise byte order.
   Every selected file is read; files that vanished or are not UTF-8 are
   skipped, as the engine does.
4. One bridge process receives the artifact source and the whole batch. It
   verifies the source against the recorded hash, loads it from memory under
   a virtual module name, builds one semantic provider, indexes the whole
   batch in workspace mode, skips the files the selector does not match, and
   transforms the rest from the content it was given. Snapshot semantics: no
   transform sees another transform's edits, unlike the workflow engine, which
   writes each file before moving to the next. Disk reads inside the sandbox
   (`jssgTransform`, the curated `fs`) see the same pre-command snapshot.
5. The returned edits (primary edits, `jssgTransform` and staged `write()`
   edits, renames) and JSON outputs are validated, merged into one write set,
   and checked for conflicts; then every destination is written and rename
   sources are removed.

Nothing touches the repository before step 5's writes. A failure in any
earlier step, or an abort, leaves every file unchanged (`failed` /
`cancelled`). A commit that stops part-way is `unknown` with the applied and
remaining paths in `error.details`.

Conflict rules: two edits to one destination, a source renamed twice, a write
to a path another edit renames away, or a rename onto an existing file that is
not itself renamed away fail the command before any write. The workflow engine
would instead let the last write win; this prototype prefers a loud failure.

A transform that writes through the curated `fs` module bypasses the batch
result and is not tracked.

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
codemod-workflow <workflow.ts> [--target <directory>] [--bridge <binary>]
```

- `--target` (default: current directory) is where `exec` runs and the root
  JSSG targets are resolved beneath.
- `--bridge` or `CODEMOD_BRIDGE_BIN` (default: the monorepo's
  `target/debug/butterflow-execution-bridge`) locates the bridge binary; the
  command fails early when it is missing.

The command prints the workflow's final value as JSON on stdout and errors on
stderr with exit code 1; it is the only place that writes to the terminal.
`SIGINT`/`SIGTERM` abort the run: the bridge process is killed and the command
in flight is recorded as `cancelled` with nothing written. The bin re-spawns
Node with `--experimental-transform-types` and a `node:module` hook that strips
types from `.ts` files under `node_modules`, then loads the workflow through
`loadWorkflow`, so it works both from this checkout and from a workspace or
package installation of `@codemod.com/orchestration` (the e2e suite installs
a copy under a consumer's `node_modules`, with `esbuild` and `typescript`
beside it, to prove it). The workflow module and the sources it imports are
expected to be ESM. No build output is written to disk. This is a
trusted-local runner: it does not sandbox the workflow body and makes no claim
about untrusted registry packages.

Remaining work before Solid Migration Assistant can move onto this package:

- add the package to that repository and author its workflow with inline
  transforms and structured outputs; helpers it shares between transforms
  must be imported modules, not top-level declarations of the workflow file;
- ship the bridge binary with the package or as a `codemod` subcommand; today it
  must be built from this monorepo and pointed at with `--bridge`;
- decide the cross-file edit policy Solid needs: today every transform sees
  the pre-command snapshot and two edits of one file in one command fail the
  command instead of chaining or merging;
- an AI executor adapter, `pipe()`, approvals, and durable history remain
  unimplemented, so any Solid step that needs them stays in YAML;
- restricted QuickJS execution of the workflow body and registry loading are
  still required before TypeScript workflows become an untrusted registry
  format; the build split already keeps transform artifacts independent of
  the workflow bundle so each can get its own bindings.

Prototype limitations of the build step: sandbox errors report positions in
the bundled artifact (named `<name>.jssg.js`), not the workflow source; a
module is split the first time Node loads it in a process, so a second
`loadWorkflow` of an already-imported module collects no artifacts; modules
loaded lazily after `loadWorkflow` returns are not split.

The programmatic API is:

```ts
import { BridgeExecutor, MemoryHistoryStore, loadWorkflow, run } from "@codemod.com/orchestration";

const { exports, artifacts } = await loadWorkflow("./workflow.ts");
const controller = new AbortController();
const executor = new BridgeExecutor({
  bin: "target/debug/butterflow-execution-bridge",
  cwd: repoDir, // exec cwd and JSSG target root
  artifacts, // built transforms by hash; their source never enters history
  events: sink, // optional: bridge.spawned events
});
const history = new MemoryHistoryStore();
const first = await run(exports.default, { executor, history, signal: controller.signal });
const again = await run(exports.default, { executor, history: MemoryHistoryStore.fromJSON(history.serialize()) });
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
ones, which is how a crashed run resumes. A `cancelled` or `unknown`
completion is recorded like any other and replays as recorded. A JSSG
command's identity includes its artifact hash, so editing a transform (or a
helper it bundles) replays as `changed`, while moving the checkout does not.

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
move to Rust one at a time without changing workflow source. `exec` has a
bridge adapter, local `jssg` has the TypeScript orchestrator around the Rust
batch, and `ai` remains protocol-only (the executor answers `failed` with "no
executor adapter"); the harness can still script any operation in unit tests.
`OperationExecutor.execute` takes an optional `AbortSignal`.

## Commands

```
pnpm install
pnpm --filter @codemod.com/orchestration test          # fast TS tests, no Rust (fake bridge)
pnpm --filter @codemod.com/orchestration typecheck
pnpm --filter @codemod.com/orchestration test:e2e     # builds only the bridge crate, then cross-language tests
cargo test -p butterflow-execution-bridge              # protocol, batch, binary, walker and language contracts
```

The full `codemod` CLI is never built or used by this package.

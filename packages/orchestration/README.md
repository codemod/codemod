# @codemod.com/orchestration (prototype)

TypeScript-first prototype of the Codemod orchestration runtime. Workflows are
plain async TypeScript; calling a runnable creates a command, awaiting it inside
a workflow issues it, and every issued command is recorded in an append-only
history and replayed from that history on later runs. `shell` runs through a
small Rust bridge over `butterflow_runners::DirectRunner`, and `agent` runs
through the same bridge on the built-in Butterflow agent (`codemod-ai`) or,
when the step asks for it, the installed Claude Code or Codex CLI.
`assessment` is file-oriented: it selects files using JSSG-style
include/exclude globs, reads each file, and asks a System One model (TypeSafe
Jev by default) typed questions about each file individually, from the host
process, with bounded concurrency and no repository access beyond the matched
files. JSSG transforms are
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
  src/index.ts           public API: re-exports from the folders below
  src/core/              plain data contracts shared by every layer
    protocol.ts          versioned JSON OperationRequest / OperationCompletion (v8)
    assessment.ts        assessment questions, answers, and their validation
    json.ts              Json type and canonical JSON (command identity)
    paths.ts             safe-relative-path rules and root containment (realpath)
    history.ts           HistoryStore seam + MemoryHistoryStore
    events.ts            EventSink seam (+ bridge.spawned, scheduler.*)
    errors.ts            error classes thrown across layers
  src/authoring/         what workflow code writes; builds commands, executes nothing
    runnable.ts          callable shell / jssg / agent / assessment descriptors (typed via Standard Schema)
    command.ts           Command: one invocation as data, awaitable inside a workflow
    context.ts           the active workflow runtime (AsyncLocalStorage in the Node prototype)
    composition.ts       sequence(...) and parallel(...) static graph nodes + IR
    dynamic.ts           explicit arbitrary-TypeScript dynamic nodes
    target.ts            validation and normalization of a JSSG invocation Target
    schema.ts            minimal Standard Schema surface and guard(...)
    transform.ts         author-facing transform/selector types per language (type-only)
  src/bundle/            splits inline JSSG transforms out of workflow modules
    build.ts             build step: extract inline transforms, bundle, rewrite, loadWorkflow
  src/execution/         how one OperationRequest becomes an OperationCompletion
    executor.ts          OperationExecutor seam + BridgeExecutor (shell, jssg, agent, assessment)
    assessment.ts        executeAssessment: one TypeSafe SDK systemOne() call per command
    scheduler.ts         AdmissionScheduler: weighted, bounded admission at the executor
    bridge.ts            spawnBridge: one bridge process per request over exchange files
    exchange.ts          host-owned exchange directory, exclusive request, no-follow response read
    process-tree.ts      best-effort process-tree kill on abort, timeout, and host exit
    jssg.ts              executeJssg: artifact, select, batch, validate, stage, commit, classify
    files.ts             file selection with the engine's walker semantics (npm `ignore`)
    languages.json       language -> extensions, pinned to the engine table by a cargo test
  src/runtime/           runs a workflow: issue commands, replay or execute, record history
    run.ts               run lifecycle, history, and command execution
    gate.ts              CommandGate seam + ReplayGate (replay matching, errors)
  src/host/              entry points that assemble a run
    cli.ts               experimental local runner behind bin/codemod-workflow.mjs
    harness.ts           test harness with scripted completions (`./harness` export)
    dashboard/           local run dashboard behind `--dashboard` (`./dashboard` export)
      monitor.ts         RunMonitor: one run's projected, sequenced envelopes + snapshot
      session.ts         DashboardSession: every run of one configuration; start/restart/history
      api.ts             routes, JSON endpoints, and SSE frames as plain functions
      server.ts          loopback HTTP binding of api.ts
      ui.html            the page, dependency-free
  fixtures/protocol      shared JSON fixtures checked by TS and Rust tests
  fixtures/walker        file-walker contract checked by TS and Rust (engine walker) tests
  tests/                 unit tests (fast, no Rust; fake bridge) + bridge.e2e.test.ts + *.live.test.ts (opt-in)
  bin/                   codemod-workflow.mjs and its node_modules type-stripping hook
crates/execution-bridge/ protocol structs, shell through DirectRunner, one JSSG batch, agent via codemod-ai or an external CLI
  src/main.rs            `butterflow-execution-bridge <request.json> <response.json>`
```

Dependencies between `src/` folders point one way; a folder imports only from
the folders on its row (`tests/layers.test.ts` enforces this):

```
core       (nothing)
authoring  core
bundle     core
execution  core, bundle
runtime    core, authoring, bundle, execution
host       core, authoring, bundle, execution, runtime
```

`authoring` and `execution` never import each other: commands meet executors
only in `runtime`, through the wire types in `core/protocol.ts`. `src/index.ts`
sits above every folder and re-exports the public API.

## Authoring

```ts
import { dynamic, jssg, parallel, sequence, shell } from "@codemod.com/orchestration";
import { rewriteSignal } from "./helpers.ts"; // bundled into the transform

const inspect = shell({ name: "inspect", command: "node inspect.js", output: Project });
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

export default dynamic(async () => {
  const project = await inspect();
  if (project.needsMigration) await migrate({ input: project });
  return project;
});

// No-input invocations ignore preceding outputs.
export default sequence(rename(), updateImports(), format());
// A parallel tuple can flow into an explicit workflow computation.
export default sequence(
  parallel(countTodos(), countFixmes()),
  dynamic((counts) => counts.reduce((total, count) => total + count, 0)),
);

// JSSG invocations carry a file target; shell, agent, and assessment never do
const web = { root: "apps/web", include: ["src/**"], exclude: ["**/generated/**"] };
export default sequence(rename({ target: web }), updateImports({ target: web }), format());
export default parallel(transformA({ target: web }), transformB({ target: web }));
export default dynamic(async () => {
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
  `migrate({ input, target, id })`. Parentheses consistently mean invocation;
  adding `id` or `target` only adds metadata. An explicit `input` binds data.
  Creating a command does nothing. Awaiting it
  inside a workflow body issues it (replay or execute) and returns its typed
  output; awaiting it anywhere else rejects with `NoActiveRunError`. A
  command issues at most once per run however often it is awaited.
- A bare runnable may be the root executable, so a one-step module can simply
  `export default shell({...})` or `export default jssg({...})`. Static
  `sequence(...)` and `parallel(...)` members always use invocation syntax.
- `shell` and `agent` invocations accept `id` and `input` only.
  `assessment` is file-oriented and returns `Array<{ file, assessment }>`;
  it accepts `id` and `input` but not `target`.
  `jssg` invocations also accept `target`. A `target` on any of them throws
  `TargetValidationError` when the command is created; any other unknown field
  throws `InvocationError`.
- `shell` output: with an `output` schema, the runner's returned text is parsed
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
- `sequence(...)` and `parallel(...)` are static graph data. They can be root
  executables; nodes that require no flowing input are also awaitable inside a
  workflow. Input-requiring nodes must be nested where a preceding stage
  supplies that input. Use invocation syntax in static plans: `step()` consumes
  flowing input when the step has an input schema and ignores it otherwise.
  `step({ id, target })` behaves the same. Only `step({ input })` binds input and
  ignores the preceding value.
  `sequence` returns only its final output. `parallel` passes the same
  input to every member and returns a tuple in declaration order. It accepts
  members spread or as one dynamic array.
- `dynamic(...)` explicitly marks arbitrary TypeScript. It can be a root or a
  stage in either static node; raw functions are rejected. Static IR leaves
  workflow stages opaque until workflow bundling and sandboxing are added.
- `parallel(...)` is eligibility, not a worker count. Members may overlap; how
  many actually do is the runtime's decision, so a group may hold any number of
  independent members and there is no author-facing concurrency knob.
- History marks commands issued under static parallel scopes as concurrent.
  Replay therefore accepts sibling issue orders caused by completion timing,
  while still requiring the same command identities, contents, and outputs.
- `parallel(...)` is also an author assertion about writes. The prototype
  overlaps whole operations and does not implement per-file locking. Do not
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
  `src/execution/languages.json`, which `cargo test -p butterflow-execution-bridge`
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
it runs (`src/bundle/build.ts`):

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

## Agent and assessment steps

```ts
import { agent, assessment, dynamic } from "@codemod.com/orchestration";

// File-oriented assessment. Each matched file is assessed individually by a
// System One model. The model receives the file's path, content, and optional
// validated input as state automatically — `ask` defines QUESTIONS ONLY.
const assessSources = assessment({
  name: "assess-sources",
  include: ["src/**/*.ts"],
  exclude: ["**/*.d.ts", "**/*.generated.ts"],
  // model: "jev-1.13.0", // optional pin; default TYPESAFE_DEFAULT_MODEL, else jev-latest
  ask: ({ file }) => ({
    route: {
      type: "choice" as const,
      instructions: `Which migration route best fits ${file.path}?`,
      criteria: {
        codemod: "A mechanical AST transformation is sufficient",
        agent: "Repository context or coordinated edits are needed",
        manual: "The evidence is insufficient for safe automation",
      },
    },
    risk: {
      type: "score" as const,
      instructions: "How risky is automatic migration of this file?",
      criteria: ["Low", "Moderate", "High", "Manual review required"],
    },
    safeToAutomate: {
      type: "noul" as const,
      instructions: "Is there enough evidence to automate this file's migration?",
      criteria: {
        true: "The migration can be attempted and verified automatically",
        false: "A person should inspect the file before any write",
      },
    },
  }),
});

const finish = agent({
  name: "finish-migration",
  prompt: "Finish the migration in the given package and make its tests pass.",
  input: Change,
  // Omit for the builtin agent; or run the installed, logged-in Codex CLI in its sandbox:
  backend: { kind: "codex", sandbox: "workspace-write" },
});

export default dynamic(async () => {
  // Returns Array<{ file: string; assessment: AssessmentResult<Q> }>,
  // one entry per matched file in deterministic selector order.
  const results = await assessSources();

  // Routing policy is workflow code: the assessment only reports probabilities.
  const anyManual = results.some(
    (r) => r.assessment.answers.route.choice === "manual"
      || r.assessment.answers.route.confidence < 0.75,
  );
  if (!anyManual) {
    await finish({ input: change });
  }
  return {
    files: results.length,
    manual: anyManual,
    routes: results.map((r) => ({ file: r.file, route: r.assessment.answers.route.choice })),
  };
});
```

`assessment({ name, include, exclude?, ask, input?, model? })` is a
file-oriented, read-only System One judgment. It selects files using
JSSG-style include/exclude globs, reads each file's content, and runs one
TypeSafe assessment per file with bounded concurrency through the scheduler.
It follows the TypeSafe System One API (<https://docs.typesafe.ai/api>) and
calls it through TypeSafe's official JavaScript SDK:

- **File selection.** `include` (required, non-empty) and `exclude` (optional)
  are glob patterns relative to `process.cwd()`. They use the same JSSG-style
  walker (`src/execution/files.ts`) with gitignore semantics: hidden files
  visited, symlinks skipped and never followed, `.ignore`/`.gitignore`/
  `.git/info/exclude` and global git excludes honored, include/exclude globs
  taking precedence over ignore files. Order is deterministic component-wise
  byte order. Each selected file is read as UTF-8; files that vanished or
  cannot be read are silently skipped.
- **`ask`** is a function that receives `{ file, input }` per file (where
  `file` is `{ path, content }` and `input` is the validated workflow input)
  and returns QUESTIONS ONLY. It may produce dynamic criteria per file (e.g.
  choice options derived from the file's content). The model state is assembled
  automatically as `{ file: { path, content }, input? }` — `ask` never
  provides state.
- **`include`/`exclude` is the privacy boundary.** Only matched file contents
  are sent to the model. Assessment is read-only and tool-free.
- **Result** is an ordered `Array<{ file: string; assessment: AssessmentResult<Q> }>`,
  preserving the deterministic selector order regardless of completion order.
  Each file's `assessment` contains `{ model, answers, usage }`, typed from the
  questions. Each file's response is validated against the exact questions
  resolved for that file.
- **Concurrency** is bounded by the scheduler (weight 1 per file, same as any
  assessment command). Files are issued concurrently through `runtime.issue()`
  and results are collected with `Promise.allSettled`.
- **Command IDs.** A single-file result uses the assessment name as the command
  id. Multi-file results use `${name}:${file.path}` per file.
- **Replay** uses the recorded operation and result without rereading files.
  The command records `state`, `questions`, and pinned `model` as content, so
  changing any of them replays as `changed`.
- **Repository-level assessment is intentionally not supported.** To assess a
  summary (build output, CI log, aggregated metrics), have a preceding shell
  or dynamic step materialize a single summary file and assess that file.
- `questions` are named. `choice` picks one of the `criteria` keys (at least
  two; values are descriptions or `null`); `score` rates against ordered
  `criteria` levels (at least two); `noul` is a yes/no question with optional
  `criteria: { true, false }`. Instructions and descriptions may be text or
  JSON. Malformed questions throw when `toOperation` is called (i.e. when the
  command is issued), since questions may depend on runtime input. Question
  ids and choice options may not be `__proto__`, `constructor`, or
  `prototype`.
- The per-file result is `{ model, answers, usage }`, typed from the questions:
  a choice answer has `choice` (one of the declared options), `probabilities`
  per option, and `confidence`; a score answer has `score` (may fall between
  levels), `legend`, `probabilities` per level, and `confidence`; a noul
  answer has `noul`, the probability of yes. `model` is the versioned model
  that answered (e.g. `jev-1.13.0` for `jev-latest`) and `usage` is
  `{ inputTokens?, outputTokens? }`, each count present only when the API
  reported it. Answers are validated against the concrete resolved questions
  both when the API responds and when a result is decoded (replay included);
  scores, probabilities, and confidence may exceed their range by
  floating-point noise (at most `1e-6`) and are kept unmodified.
- It decides nothing. Thresholds, fallbacks, and routing are workflow code.
- `BridgeExecutor` runs it in the host process through the official TypeSafe
  JavaScript SDK, `@typesafe-ai/sdk` (0.6.x): one `TypeSafeClient.systemOne()`
  call per command, no bridge process. The SDK owns authentication, the
  `TYPESAFE_API_KEY` (required), `TYPESAFE_BASE_URL` (default
  `https://api.typesafe.ai`), and `TYPESAFE_DEFAULT_MODEL` (default
  `jev-latest`) fallbacks, request encoding, HTTP transport, and response
  decoding. Explicit settings win over the environment:
  `new BridgeExecutor({ ..., assessment: { apiKey, baseURL, defaultModel,
  timeoutMs, retry } })`. SDK logging is turned off (`logLevel: "off"`,
  regardless of `TYPESAFE_LOG_LEVEL`), so assessments never write to the
  terminal.
- Retries and timeouts are the SDK's; nothing in this package retries an
  assessment, so there is one retry layer. With the SDK's default
  `RetryPolicy` that is up to 2 retries for HTTP 408, 429, and 500-599,
  connection failures, and attempt timeouts, with exponential backoff (500 ms
  to 5 s, 25% jitter) or the server's `retry-after-ms` / `Retry-After` when it
  is at most 60 s. `retry` passes SDK `RetryPolicy` overrides through
  (`{ maxRetries: 0 }` disables retries). `timeoutMs` (default 30 s) is the
  SDK's per-attempt timeout; the SDK has no total budget, so the worst case is
  `(maxRetries + 1) * timeoutMs` plus backoff waits. The run's `AbortSignal`
  is handed to the SDK, which cancels the in-flight attempt and any backoff
  wait. The evaluation is read-only, so repeating it is safe.
- Executor options are checked before any request: an explicit `undefined`
  or blank string means "use the SDK fallback". This package adds only what
  the SDK leaves unbounded: `timeoutMs` at most Node's timer limit (about 24.8
  days), `retry.maxRetries` at most `MAX_ASSESSMENT_RETRIES` (10), and an
  effective base URL (option, environment, or default) that is an absolute
  `http(s)` URL without query or fragment. The SDK validates everything else
  (positive timeout, retry settings) and refuses a missing API key when the
  client is built. Any of these is a `failed` completion with
  `phase: "config"` and no request, never a thrown error.
- Failures are `failed` with `error.details`, mapped from the SDK's error
  classes (not from messages): `phase` is `config` (the checks above),
  `request` (`APIConnectionError`;
  `APITimeoutError` adds `timedOut: true`), `http` (`APIError`, with `status`,
  the SDK's `requestId` and parsed `body` when present, and `retryAfterMs` for
  a 429 that sent one), `response` (a 2xx body, JSON or not, that does not
  answer the questions: the SDK returns the body without checking it, so it is
  normalized and validated here), or `internal` (anything else). For
  `request` and `http`, `retryable` says whether the SDK's retry policy covers
  that error, meaning its retries were exhausted; the SDK does not report how
  many attempts it made. An abort (`APIUserAbortError` or the run's signal) is
  `cancelled`. Nothing is ever `unknown`: an assessment cannot have changed
  the repository.

`agent({ name, prompt, input?, output?, backend? })` hands a task to an agent
loop chosen by `backend`, a discriminated union that is recorded in the command
(so changing the backend or any of its settings replays as `changed`):

```ts
// builtin (the default): codemod-ai, the Rig loop behind YAML `ai` steps; uses LLM_API_KEY
agent({ name: "fix", prompt, backend: { kind: "builtin", tools: ["str_replace_based_edit_tool", "glob"], maxSteps: 20 } });
// claude-code: the installed, logged-in `claude` CLI; only its tool set is configurable
agent({ name: "fix", prompt, backend: { kind: "claude-code", tools: ["Read", "Edit", "Write"] } });
// codex: the installed, logged-in `codex` CLI; only its sandbox is configurable
agent({ name: "fix", prompt, backend: { kind: "codex", sandbox: "workspace-write" } });
```

- Each backend accepts only the settings it enforces. `builtin`: `tools`
  (default `DEFAULT_BUILTIN_AGENT_TOOLS`: `str_replace_based_edit_tool`,
  `json_edit_tool`, `glob`, `sequentialthinking`, `task_done`; no `bash`, no
  `mcp_tool`, which starts arbitrary server processes, no `ckg_tool`, which
  writes a database into the repository) and `maxSteps` (the agent's own limit,
  30, when omitted). `claude-code`: `tools` from `Read`, `Edit`, `Write`,
  `Glob`, `Grep`, `Bash` (default all but `Bash`); Claude Code has no
  enforceable turn limit here. `codex`: `sandbox`, `read-only` or
  `workspace-write` (default `workspace-write`); `danger-full-access` is not
  representable and Codex's own tools cannot be listed. `maxSteps` on an
  external backend, `tools` on `codex`, an unknown backend, or an unknown
  setting throws when the step is defined; the bridge rejects the same
  requests as parse errors. Top-level `tools`/`maxSteps` are not accepted.
- The bridge runs the backend in `--target`, so it may change files. Validated
  input is appended to the prompt as JSON; the prompt goes to external CLIs on
  stdin, never in argv.
- The result is `{ text }` for every backend: the builtin agent's final
  response, Claude Code's `result`, or Codex's last message. Progress and
  event streams are not kept. With an `output` schema the operation carries
  `responseFormat: "json"`, the bridge appends an instruction to reply with a
  single JSON value, and the text is parsed (bare JSON, or the content of
  exactly one ```json or bare ``` fence) and validated. A decode error names
  the step and quotes the start of the response. The response is recorded
  before it is decoded: the agent has already run, and replaying the same
  history fails the same way; retrying needs a new command id or history.
- `builtin` configuration is the engine's AI step convention: `LLM_API_KEY`
  (required), `LLM_PROVIDER` (`openai`, `anthropic`, `google_ai`,
  `azure_openai`; default `openai`), `LLM_MODEL` (default `gpt-4o`),
  `LLM_BASE_URL` (default per provider).
- External harnesses (`claude-code`, `codex`) are different programs, not
  alternative models for codemod-ai: they own their agent loop, prompts, and
  tool implementations, authenticate with the CLI's existing local login and
  spend that subscription's quota, may load repository instructions
  (`CLAUDE.md`, `AGENTS.md`) from the target, and do not exercise codemod-ai or
  Rig at all. They never receive `LLM_API_KEY`. The bridge checks login state
  first (`claude auth status --json`, keeping only `loggedIn`; the exit code of
  `codex login status`) and never reads credential files. The check runs from
  an empty private directory, never the target, with exactly the environment
  the task gets, so repository settings, instructions, or hooks cannot change
  its result. Codex additionally requires the target to be inside a git
  repository. The CLI is looked up on absolute `PATH` entries only; empty,
  `.`, and relative entries are ignored so a target cannot shadow it.
- Non-interactive, no-prompt invocation (flags taken from each CLI's `--help`;
  `--dangerously-skip-permissions` and
  `--dangerously-bypass-approvals-and-sandbox` are never used):
  - `claude -p --output-format json --no-session-persistence --restricted
    --strict-mcp-config --disable-slash-commands --permission-mode dontAsk
    --permission-prompts none --tools <tools> --allowedTools <tools>`:
    `--restricted` ignores user, project, and local settings files (so their
    hooks and plugins), confines file tools to the working directory, and
    refuses bypass; no MCP servers load; only the listed tools exist and they
    are pre-approved; anything that would still ask is denied, so nothing
    waits for a person. Managed (policy) settings still apply.
  - `codex exec --sandbox <mode> -c approval_policy="never"
    -c shell_environment_policy.inherit="core"
    -c sandbox_workspace_write.exclude_slash_tmp=true --ephemeral
    --ignore-user-config --ignore-rules --color never --json -C <target> -`:
    Codex's OS sandbox confines writes (and, by default, network) for commands
    it runs; approvals never block; commands see only Codex's core variables
    (`PATH`, `HOME`, `TMPDIR`, ...) minus its default `*KEY*`/`*SECRET*`/
    `*TOKEN*` excludes; `/tmp` is not writable and `TMPDIR` is a private empty
    directory the bridge creates for the run; `config.toml` (MCP servers,
    profiles, trust overrides, `notify`) and execpolicy `.rules` files are not
    loaded, while auth still comes from `CODEX_HOME`. The final text is the
    last `agent_message` in the `--json` event stream, scanned line by line
    with lines over 16 MiB skipped; no output file path is given to Codex.
  - Both CLIs start with a private `TMPDIR` and without any variable whose
    name looks like a credential or provider setting (`*_API_KEY`, `*TOKEN*`,
    `*SECRET*`, `AUTH`, `ANTHROPIC_*`, `OPENAI_*`, `CODEX_*` except
    `CODEX_HOME`, `AWS_*`, ...), so their tools cannot inherit one.
    `BridgeOptions.env` containing such a name is refused for external
    backends as a `config` failure before anything starts.
- Host wall-clock limit: `new BridgeExecutor({ ..., externalAgentTimeoutMs })`,
  default `DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS` (30 minutes), applies to
  `claude-code` and `codex` bridges. When it passes, the bridge's process tree
  is killed as on abort and the command is `unknown` with
  `details: { phase: "execute", timedOut: true, repositoryMayBeModified: true }`.
  It is host configuration, not part of history; an invalid value is a
  `config` failure. Inside the bridge, a CLI that exits while a descendant
  still holds its stdout is read for at most 2 more seconds.
- Outcomes. `failed` with `details: { phase: "config",
  repositoryMayBeModified: false }` means the agent never started: no
  `LLM_API_KEY` or repeated tools or invalid `maxSteps` (builtin); the CLI
  missing from PATH, not logged in, its login check failing or timing out
  (30 s), the process failing to spawn, (codex) no git repository, a
  credential-looking `BridgeOptions.env` name, an invalid
  `externalAgentTimeoutMs`, or no usable exchange directory.
  `failed` with `phase: "execute", repositoryMayBeModified: true` means the
  agent started and did not produce a successful final response (a model or
  tool error, a non-zero CLI exit, an error result, a missing final message),
  so files may have changed. `unknown` (`repositoryMayBeModified: true`) means
  the host lost track of a started bridge: aborted, killed, or timed out after
  spawn, no response, a response that is a symlink, not a regular file, or
  not a valid completion, or a bridge that could not start. `cancelled` only happens for an
  abort before the bridge was spawned.

Security model of `agent` (a trusted-local prototype, not a sandbox):

- Environment: the bridge does not inherit the host environment. It gets
  `AGENT_ENV_ALLOWLIST` (process basics such as `PATH`, `HOME`, `TMPDIR`,
  `LANG`, `LC_*`, the Windows equivalents, proxy and CA variables,
  `CLAUDE_CONFIG_DIR` and `CODEX_HOME` so a non-default CLI home is found, and
  for `builtin` only `LLM_PROVIDER`, `LLM_MODEL`, `LLM_BASE_URL`) plus
  `BridgeOptions.env`, which wins (names compared case-insensitively on
  Windows). External backends get no `LLM_*` variables and no stdin secret.
  `TYPESAFE_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, cloud, CI, and
  registry credentials stay out unless passed explicitly.
- The API key: `BridgeExecutor` never puts `LLM_API_KEY` in the bridge's launch
  environment, so `ps eww` or `/proc/<pid>/environ` of the bridge does not show
  it. It sends `{"LLM_API_KEY": ...}` on the bridge's stdin with
  `CODEMOD_BRIDGE_SECRETS=stdin`; it never enters the request file or
  history. The bridge binary reads it before any thread starts and removes
  `LLM_API_KEY` and the marker from its environment, so tool processes do not
  inherit them. When the bridge is launched another way with `LLM_API_KEY` in
  its environment, removal only affects inheritance: the launch environment
  stays readable through `/proc` and `ps` for the process's lifetime. Either
  way the key is in the bridge's memory, and the host's own environment (with
  whatever the user exported) is readable by same-user processes. Opting into
  `bash` or `mcp_tool` therefore lets the model read anything the user can,
  including other processes' environments where the OS permits. The Rust
  library's `execute()` reads `LLM_API_KEY` from the environment and leaves it
  there; embedders must scrub it themselves.
- Builtin tools are not confined. The file tools accept any absolute path the
  user can read or write, not just `--target`, and the editor's directory view
  shells out to `find`. `bash` and `mcp_tool` run arbitrary commands with
  network access. There is no filesystem, process, or network sandbox.
- Bridge exchange files (request and response) live outside the target in a
  host-owned per-user directory: `CODEMOD_BRIDGE_EXCHANGE_DIR` if set (used as
  is or the bridge is not started), else `~/Library/Caches/codemod/bridge`
  (macOS), `$XDG_RUNTIME_DIR/codemod/bridge` or
  `${XDG_CACHE_HOME:-~/.cache}/codemod/bridge` (other POSIX),
  `%LOCALAPPDATA%\codemod\bridge` (Windows), falling back to
  `<os tmpdir>/codemod-bridge-<uid>` when those cannot be created. The root
  must be a real directory owned by the user with mode 0700 (tightened when
  owned but looser) and not inside the target; each request gets a fresh
  0700 directory, removed afterwards. The request is created exclusively
  (0600); the bridge creates the response exclusively (`O_CREAT|O_EXCL`,
  0600), so a planted file or symlink is never written through; the host
  opens it with `O_NOFOLLOW` and accepts only a regular file up to 512 MiB.
  None of these locations is a Codex writable root. An opted-in Claude Code
  `Bash`, or builtin `bash`, runs unsandboxed as the same user and can still
  reach them; Windows has no ownership or mode check.
- External harness confinement is whatever the CLI enforces: Claude Code's
  `--restricted` file-tool confinement and permission rules (an opted-in
  `Bash` runs commands as the user, with network access), and Codex's OS
  sandbox (`read-only` or `workspace-write`). The bridge cannot verify either,
  and a CLI update can change them. Both CLIs read the user's own login state
  and may load `CLAUDE.md`/`AGENTS.md` from the repository, which steers the
  agent like any prompt text. `--restricted` ignores project settings, so a
  repository `.claude/settings.json` `apiKeyHelper` or hook is not loaded
  (the claude-code live test asserts both left no trace); Codex ignores user
  and project `config.toml` under `--ignore-user-config` (the codex live test
  asserts a project `notify` command did not run).
- Cancellation kills the bridge and, best effort, its process tree. On POSIX
  the bridge leads its own process group: the group is stopped, one `ps`
  snapshot (1 s timeout) finds descendants that left the group by parent id
  (such as the bash tool's session), those are stopped parents first and
  killed children first, then the group is killed. A stopped parent cannot
  reap an exited child, so its PID cannot be reused before the kill; the only
  reuse window is between the snapshot and stopping a parent outside the
  group. On Windows `taskkill /T /F` is used. A descendant that daemonized, or
  a sandbox that hides the process list (only the group is reached then), can
  leave processes behind.
- Host lifetime: bridges are detached, so they do not receive the terminal's
  signals. `codemod-workflow` turns SIGINT, SIGTERM, and SIGHUP into an abort,
  and any normal exit of the host process kills bridges still running.
  Programs that embed `run()` or `BridgeExecutor` must do the same: abort the
  run's `signal` on SIGINT, SIGTERM, and SIGHUP (or exit normally). If the
  host is killed with SIGKILL or crashes, nothing runs and bridges and their
  agents can keep working orphaned. There is no parent-death channel: a bridge
  that noticed its host dying could kill its own group, but not the
  descendants outside it without the same process listing, so it would be a
  partial cleanup and is deliberately not implemented.

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
codemod-workflow <workflow.ts> [--target <directory>] [--bridge <binary>] [--input <json>] [--dashboard]
```

- `--target` (default: current directory) is where `shell` and `agent` run
  and the root JSSG targets are resolved beneath.
- `--bridge` or `CODEMOD_BRIDGE_BIN` (default: the monorepo's
  `target/debug/butterflow-execution-bridge`) locates the bridge binary; the
  command fails early when it is missing.
- `--input <json>` supplies the root input when the workflow declares one.
- `--dashboard` hosts the local run dashboard (below), prints its URL on
  stderr, and keeps the command alive after the run so the page can start it
  again; `Ctrl-C` ends it.

The command prints the workflow's final value as JSON on stdout and errors on
stderr with exit code 1; it is the only place that writes to the terminal.
`SIGINT`/`SIGTERM`/`SIGHUP` abort the run: the bridge process tree is killed and the
command in flight is recorded as `cancelled` with nothing written (an `agent`
already running is recorded as `unknown`, since it may have edited files). (In dashboard mode
the same holds for the newest run when the host exits; see Dashboard.) The bin re-spawns
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
- approvals and durable history remain unimplemented, so any Solid step that
  needs them stays in YAML;
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
  cwd: repoDir, // shell and agent cwd, JSSG target root
  artifacts, // built transforms by hash; their source never enters history
  events: sink, // optional: bridge.spawned events
  assessment: { timeoutMs: 10_000 }, // optional: TypeSafe SDK settings over TYPESAFE_*
});
const history = new MemoryHistoryStore();
const first = await run(exports.default, { executor, history, signal: controller.signal });
const again = await run(exports.default, { executor, history: MemoryHistoryStore.fromJSON(history.serialize()) });
// again.replayed === true, nothing was executed
```

## Dashboard

`codemod-workflow <workflow.ts> --dashboard` serves a live view of the run on
`http://127.0.0.1:<port>/` and keeps the command alive after the run so the
page can start it again:

```sh
node bin/codemod-workflow.mjs demo/05-composed.ts --target /tmp/codemod-demo --dashboard
# stderr: dashboard: http://127.0.0.1:52107/
# stderr: run 1 started
# stderr: run 1 done in 4.2s
# ...Ctrl-C: stdout gets the newest run's JSON result, then the command exits
```

In dashboard mode the command is a host. Each run's start and outcome is one
line on stderr (`run 2 started`, `run 2 done in 4.2s`, `run 2 failed after
1.0s: ...`, `run 2 stopped after 0.3s`). stdout is written once, at exit, and
carries the newest run's JSON result; when that run failed or was stopped the
command exits 1 with its error, exactly as a plain run would. `Ctrl-C`
(SIGINT/SIGTERM) aborts the active run if there is one, waits for it to
settle, closes the server, and exits.

The page is written for an operator, phone first. The top answers what the
workflow is, whether it is Running, Paused, Done, Failed or Stopped, what is
running **Now** and what is **Waiting**, with one large Pause/Resume button
(pinned to the bottom of the screen on a phone), a smaller **Restart** under
it while the run is open, and **Run again** in its place once the run is
over. Everything diagnostic is behind a single **Details** disclosure, and
earlier runs behind a collapsed **Recent runs** one. How the plain words map
to the runtime:

- **Run status.** Running and Paused describe admission while the run is
  open. Paused means nothing more is admitted; steps already admitted keep
  running, and the page says so ("Letting 2 running steps finish"). Done is
  `completed` (`run()` resolved, including a run answered entirely from
  history), Failed is `failed` (`run()` rejected, with its message under the
  status), Stopped is `cancelled` (`run()` rejected after SIGINT/SIGTERM or a
  Restart aborted the run). Pause and Resume are hidden, and refused by the
  server, once a run is over; Run again takes their place.
- **Steps.** The serializable IR of the root executable (`executableIr()`),
  known before anything runs, as an indented list: `sequence` groups are
  labelled "In order" and `parallel` groups "At the same time"; a group nested
  in a group of the same kind is flattened into it, and a one-member group is
  drawn as its member. A `dynamic()` stage is a "Chosen while running" row.
  Commands the static plan does not name (everything issued inside a
  `dynamic()` body) are listed under that row when the plan has exactly one
  dynamic stage, and under "Added while running" otherwise.
- **Per-step state.** Not started -> Waiting (`scheduled` or `queued`) ->
  Running -> Done / Failed / Stopped / Result unknown (`succeeded` / `failed` /
  `cancelled` / `unknown`). Commands answered from history read "Done earlier"
  (or their recorded outcome plus "earlier"). A step still open when the run
  ends reads "Didn't finish". Failed steps show their message and JSSG phase
  inline, and are also listed under **Failed** while the run is open; never
  the operation content.
- **Details.** The run number and id, start time, the admission limit and how
  much of it is in use, and a plain-language activity list for the current
  run; process ids and weights are not shown.
- **Pause / Resume.** Admission pause at the scheduler seam
  (`AdmissionScheduler.pause()` / `resume()`): the FIFO queue is held, admitted
  operations run to completion, and resume pumps the queue again in order.
  A pause on its own never produces a `cancelled` or `unknown` completion;
  SIGINT during a pause still cancels as it always does (queued commands are
  refused, admitted ones are killed). Pause state lives in the process only;
  there is no durable cross-process resume.
- **Restart / Run again.** Both launch a fresh run of the same loaded
  workflow, target, bridge, built artifacts, and root input, under a new run
  id with a fresh scheduler and an empty history, so nothing is replayed from
  the earlier run. Restart while a run is open first asks ("Stop what's
  running and start the workflow over?"), then aborts it exactly as SIGINT
  would (queued steps refused, running bridge processes killed), waits until
  every step has settled, and only then starts the next run: the two never
  overlap. Restart on a finished run and Run again are the same fresh launch.
  Run again while a run is open is refused (`409`) and the page points at
  Restart; a second tap during a restart joins the one in progress instead of
  starting a third run.
- **Recent runs.** The last 20 runs of this process, newest first, each with
  its number, outcome, duration, and start time. Tapping one shows its final
  snapshot read-only under a banner ("Looking at run 2, from earlier"); the
  page keeps following the current run underneath, and the banner's button
  or the big bottom button returns to it. The list lives in memory only: it is
  gone when the command exits, and it is not history in the replay sense.

The pieces (`src/host/dashboard/`):

- `RunMonitor` is the run's `EventSink`. Each `RunEvent` is projected to a
  payload without operation content (no `env`, bound inputs, or outputs; a
  failure keeps its message and phase), stamped with a process-local `runId`,
  a monotonic `seq`, and an ISO timestamp, folded into an in-memory snapshot,
  and kept in a bounded buffer (500 envelopes) for reconnects. Every envelope
  also carries the run status, the scheduler counters, and the command it
  changed, after the event, so a client applies it without its own reducer.
  `emit` is synchronous bookkeeping; a subscriber can neither block nor break
  the run. None of this touches history: command identity, `ScheduledCommand`,
  and replay are unchanged.
- `DashboardSession` is the run manager. It holds the loaded configuration
  (the executable, an executor factory, the root input, the admission
  capacity) and every run of it this process has made. `start()`,
  `restart()`, and `close()` go through one serial chain, so two taps cannot
  create two runs; each run gets a fresh `RunMonitor` under a new `runId`, a
  fresh `AdmissionScheduler`, its own `AbortController`, and a `run()` over
  an empty `MemoryHistoryStore`. `restart()` aborts the active run and awaits
  its settlement before launching; `start()` refuses with `run_active` while
  one is open; `pause()`/`resume()` act on the active run or refuse with
  `no_active_run`. It keeps at most 20 records (`runLimit`), newest first,
  never evicting a run that has not settled, exposes `runs()`,
  `snapshot(runId)`, `outcome(runId)`, and `settled(runId)`, and notifies
  subscribers with `run.created` / `run.settled`. It knows nothing of HTTP.
- `startDashboard({ session })` binds `127.0.0.1` on a free port and answers
  `GET /` (the page), `GET /api/snapshot` (the newest run), `GET /api/runs`
  (`{ current, active, runs }`), `GET /api/runs/<id>` (a retained run's
  snapshot; `404 run_not_found` otherwise), `POST /api/runs` (new run: `201`,
  or `409 run_active` while one is open), `POST /api/restart`,
  `POST /api/pause`, and `POST /api/resume` (`409 no_active_run` once the run
  is over). Controls always act on the active run, never an archived one.
  `GET /api/events` is SSE: a `snapshot` event for the current run, then one
  `event` per envelope with `id: <runId>:<seq>`. A reconnect whose
  `Last-Event-ID` names the current run gets only the missed envelopes (or a
  fresh snapshot when it is beyond the buffer); one naming an earlier run
  gets the current run's snapshot, so two runs' sequence numbers are never
  mixed. Every session notice is forwarded as an `event: run` message
  carrying the run list, and a `run.created` is followed by the new run's
  snapshot, which is how the page follows a restart without reloading. A
  stream client whose socket backlog passes 1 MiB is dropped and reconnects;
  the run never waits for it. Routes, endpoint bodies, and stream frames are
  plain functions in `api.ts` (`matchRoute`, `handleApi`, `openEventStream`),
  tested without a socket.
- Trust: v1 is local, with no authentication and no persistence. The server
  answers only requests whose `Host` is the loopback address it was bound
  under (against DNS rebinding), and control requests must send
  `Content-Type: application/json` (so a cross-site form cannot pause,
  restart, or start a run). The server lives as long as the session, that is
  until the command exits; the page then keeps what it last rendered.
- The page is one dependency-free HTML file, responsive down to a phone, in
  light and dark. It uses `EventSource` and re-renders from the snapshot plus
  envelopes; it fetches `/api/runs` for the list and `/api/runs/<id>` for an
  earlier run, and nothing else.

Programmatically, the same pieces compose around `run()`:

```ts
import { BridgeExecutor } from "@codemod.com/orchestration";
import { DashboardSession, startDashboard } from "@codemod.com/orchestration/dashboard";

const session = new DashboardSession({
  executable: workflow,
  workflow: "migrate.ts",
  executor: (events) => new BridgeExecutor({ bin, cwd, artifacts, events }),
});
const dashboard = await startDashboard({ session });
const first = await session.start(); // a RunSummary; session.settled(first.runId) resolves when it is over
// later: await session.restart(); session.runs(); session.snapshot(runId); session.outcome()
await session.close(); // aborts an active run and waits for it
await dashboard.close();
```

## Bounded admission

`parallel()` says which operations may overlap. It never says how many run at
once: that is the runtime's decision, so a group can declare thirty-seven
independent members and the host still admits a safe number of them.

Each `run()` owns one `AdmissionScheduler` (`src/execution/scheduler.ts`) and wraps the
executor it was given in a `SchedulingExecutor`. Every operation that is really
executed acquires a permit first; a replayed command never reaches the executor
and therefore consumes no capacity. Because the permit is held around
`OperationExecutor.execute`, and JSSG selection and file reading happen inside
it, a queued command holds only its `OperationRequest`, never a repository
snapshot.

The model is a weighted semaphore with a strict FIFO queue:

| operation | weight | why |
| --- | --- | --- |
| `shell`, `agent` | 1 | one child process |
| `assessment` | 1 | one HTTP request, no local work |
| `jssg` (no semantics or `"file"`) | 2 | a bridge process plus the whole selected file set in memory |
| `jssg` with `"workspace"` semantics | 4 | the batch is also parsed and indexed as one workspace |

Default capacity is `min(availableParallelism(), memory budget)` and never
exceeds the available CPU count. The memory budget is half of `totalmem()`
divided by an assumed 512 MiB per concurrent workspace pass. On a host whose
capacity is below an operation's nominal weight, that operation consumes the
whole capacity and runs alone. On a 10-core host with plenty of memory the
capacity is 10 units: two workspace passes, or ten `shell` commands, at a time.
Only the head of the queue is admitted, so a heavy command is never starved by
lighter ones behind it.

Overrides are host configuration, not authoring. They never reach a workflow
module and are not part of any command's identity or history:

- `CODEMOD_ORCHESTRATION_CAPACITY=<n>` for operators and CI;
- `run(executable, { scheduler: new AdmissionScheduler({ capacity, weights, host }) })`
  for tests and benchmarks. `scheduler.stats()` reports
  `{ capacity, used, active, queued, peakActive, peakUsed, paused }`, and the
  run's `EventSink` receives `scheduler.queued`, `scheduler.admitted`, and
  `scheduler.released`.

The host may pause admission: `scheduler.pause()` stops admitting from the
queue (new acquisitions queue even when capacity is free) while every admitted
operation runs to completion and returns its permit; `scheduler.resume()`
pumps the queue again from the head. Both are idempotent and report
`scheduler.paused` / `scheduler.resumed` to the scheduler's own `events` sink
(`new AdmissionScheduler({ events })`; the scheduler `run()` creates by default
reports to the run's sink). This is the dashboard's one control.

Cancellation splits by admission state. A command aborted while queued is
removed from the queue and completes `cancelled` without the executor ever
being called, so nothing is spawned for it. A command that was already admitted
keeps the existing behavior: the bridge process is killed and the completion is
`cancelled` or `unknown`. Permits are released on success, on a non-success
completion, on cancellation, and when the executor throws while launching.

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

The body may receive flowing data, but never a runtime context. `run()` binds
the run's runtime to the body with Node's `AsyncLocalStorage`, so commands
awaited anywhere in the body's async continuations issue to that run, two
concurrent runs never see
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
move to Rust one at a time without changing workflow source. `shell` has a
bridge adapter, local `jssg` has the TypeScript orchestrator around the Rust
batch, `agent` has a bridge adapter over `codemod-ai`, and `assessment` has a
host-side HTTP adapter (the bridge decodes it for parity and refuses it); the
harness can script any operation in unit tests (for `shell` and `agent` a
non-string value becomes JSON `stdout` / `text`).
`OperationExecutor.execute` takes an optional `AbortSignal`.

## Commands

```
pnpm install
pnpm --filter @codemod.com/orchestration test          # fast TS tests, no Rust (fake bridge)
pnpm --filter @codemod.com/orchestration typecheck
pnpm --filter @codemod.com/orchestration test:e2e     # builds only the bridge crate, then cross-language tests
cargo test -p butterflow-execution-bridge              # protocol, batch, binary, walker and language contracts
```

Live tests call real services, cost money or subscription quota, and are
opt-in, one flag per backend. Each live agent test uses a temporary git
repository, a tiny deterministic task (read `VERSION`, write `RELEASE.txt`,
answer with JSON), the backend's default settings (no shell tool, no bypass
flags), and a 5-minute timeout. Ordinary `test`
runs report them as skipped and make no request, even when credentials are
set; opting in without the required credentials fails instead of skipping.

```
# assessment() against TypeSafe (TYPESAFE_BASE_URL and TYPESAFE_DEFAULT_MODEL optional)
TYPESAFE_API_KEY=... pnpm --filter @codemod.com/orchestration test:live:assessment
#   = CODEMOD_LIVE_ASSESSMENT=1 vitest run tests/assessment.live.test.ts

# builtin agent() through the real bridge (LLM_PROVIDER, LLM_MODEL, LLM_BASE_URL, CODEMOD_BRIDGE_BIN optional)
LLM_API_KEY=... pnpm --filter @codemod.com/orchestration test:live:agent:builtin
#   = pnpm build:bridge && CODEMOD_LIVE_AGENT_BUILTIN=1 vitest run tests/agent.builtin.live.test.ts

# claude-code agent() with the installed `claude` CLI and its existing login
pnpm --filter @codemod.com/orchestration test:live:agent:claude-code
#   = pnpm build:bridge && CODEMOD_LIVE_AGENT_CLAUDE_CODE=1 vitest run tests/agent.claude-code.live.test.ts

# codex agent() with the installed `codex` CLI and its existing login
pnpm --filter @codemod.com/orchestration test:live:agent:codex
#   = pnpm build:bridge && CODEMOD_LIVE_AGENT_CODEX=1 vitest run tests/agent.codex.live.test.ts
```

The TypeSafe SDK uses the global `fetch`, which ignores `HTTPS_PROXY`; behind a
proxy add `NODE_USE_ENV_PROXY=1` (Node 24) to the assessment command.

The full `codemod` CLI is never built or used by this package.

# @codemod.com/orchestration (prototype)

TypeScript-first prototype of the Codemod orchestration runtime. Workflows are
plain async TypeScript; calling a runnable creates a command, awaiting it inside
a workflow issues it, and every issued command is recorded in an append-only
history and replayed from that history on later runs. A workflow runs in the
`quickjs` runtime (see "Runtime"): the whole bundled workflow in one QuickJS
context hosted inside the Rust `codemod` CLI (`codemod workflow run -w`), no
Node at run time. It executes every
operation kind. `shell` runs in-process
over the host's `spawn` with the semantics of
`butterflow_runners::DirectRunner`, and `agent` runs in a `codemod-agent`
runner process on the built-in Butterflow agent (`codemod-ai`) or, when the
step asks for it, the installed Claude Code or Codex CLI. `assessment` is
file-oriented: it selects files using JSSG-style include/exclude globs, reads
each file, and asks the decision model the environment names (TypeSafe Jev
over the System One API by default, or OpenAI's Decisions API) typed
questions about each file individually, in-process, with bounded concurrency
and no repository access beyond the matched files. JSSG transforms are written
inline next to the workflow, split off into standalone bundles at build time,
and orchestrated here in TypeScript (file selection, ordering, conflict
checks, transactional commit, output aggregation, failure classification)
around the shared Rust batch rules (`crates/orchestration/src/jssg_batch.rs`: containment, static
selector, semantic providers, result conversion), which the QuickJS host calls
in-process (see `PROTOCOL.md`). The evidence, problem statement, proposal,
boundaries, and migration path are summarized in `DESIGN.md`.

## Layout

```
packages/orchestration/
  DESIGN.md              problem statement, proposal, scope, and migration path
  PROTOCOL.md            the operation protocol, JSSG batch, agent runner, security model, transactions
  src/index.ts           package API: portable.ts + the assessment and agent executors
  src/portable.ts        the API the workflow bundle imports (`./portable` export)
  src/core/              plain data contracts shared by every layer
    host.ts              Host seam (fs, path, os, env, spawn, async context, clock) + install
    protocol.ts          versioned JSON OperationRequest / OperationCompletion (v8)
    assessment.ts        assessment questions, answers, and their validation
    decision.ts          decision provider, model, endpoint, and key, from the environment only
    json.ts              Json type and canonical JSON (command identity)
    paths.ts             safe-relative-path rules and root containment (realpath)
    history.ts           HistoryStore seam + MemoryHistoryStore
    events.ts            EventSink seam (command.*, run.finished, scheduler.*)
    errors.ts            error classes thrown across layers
    posix.ts             dependency-free node:path.posix port
  src/authoring/         what workflow code writes; builds commands, executes nothing
    runnable.ts          callable shell / jssg / agent / assessment descriptors (typed via Standard Schema)
    command.ts           Command: one invocation as data, awaitable inside a workflow
    context.ts           the active workflow runtime (Host async-context variable)
    composition.ts       sequence(...) and parallel(...) static graph nodes + IR
    dynamic.ts           explicit arbitrary-TypeScript dynamic nodes
    target.ts            validation and normalization of a JSSG invocation Target
    schema.ts            minimal Standard Schema surface and guard(...)
    transform.ts         author-facing transform/selector types per language (type-only)
  src/execution/         how one OperationRequest becomes an OperationCompletion
    operation-executor.ts OperationExecutor seam
    shell.ts             executeShell: in-process shell over Host.spawn (DirectRunner semantics)
    assessment.ts        executeAssessmentOperation: one decision API request per file over the Host fs
    decision/            provider adapters (systemone.ts, openai.ts) and the HTTP transport (transport.ts)
    agent.ts             executeAgent: one codemod-agent runner process per agent operation
    agent-env.ts         agentLaunch: the runner's allowlisted environment and stdin key channel
    scheduler.ts         AdmissionScheduler: weighted, bounded admission at the executor
    jssg.ts              executeJssgCommand: artifact, select, dispatch batch, validate, stage, commit
    jssg-batch.ts        the portable JSSG batch loop (over crates/orchestration/src/jssg_batch.rs)
    files.ts             file selection with the engine's walker semantics (npm `ignore`)
    languages.json       language -> extensions, pinned to the engine table by a cargo test
  src/runtime/           runs a workflow: issue commands, replay or execute, record history
    run.ts               run lifecycle, history, and command execution
    gate.ts              CommandGate seam + ReplayGate (replay matching, errors)
    capabilities.ts      runtime descriptors (quickjs) and capability checks
  src/host/quickjs/      the bundle entry and Host inside the QuickJS host (incl. fetch.ts)
  fixtures/protocol      shared JSON fixtures checked by TS and Rust tests
  fixtures/walker        file-walker contract checked by TS and Rust (engine walker) tests
  tests/                 unit tests (fast; fake agent runner) + *.live.test.ts (opt-in)
    support/             test-only Node host, scripted harness, process-tree kill (Vitest runs on Node)
crates/cli/src/commands/workflow/typescript.rs  `codemod workflow run -w workflow.ts`
crates/cli/tests/        end-to-end specs: typescript_workflow.rs, jssg_parity.rs
crates/orchestration/    the Rust side, one crate:
  src/build/             the build step: jssg() split (oxc), bundles (rolldown), cache, embedded runtime
  src/host/              the QuickJS host, run in-process by the CLI (incl. the transform pool)
  src/walker/            native file selection and reads for the host
  src/jssg_batch.rs      the JSSG batch rules the host calls (BatchPlan)
crates/orchestration-agent/  `codemod-agent`: one agent operation over stdin/stdout
```

Dependencies between `src/` folders point one way; a folder imports only from
the folders on its row (`tests/layers.test.ts` enforces this):

```
core       (nothing)
authoring  core
execution  core
runtime    core, authoring, execution
host       core, authoring, execution, runtime
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
  `src/execution/languages.json`, which `cargo test -p orchestration`
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
`Codemod` and the runtime rejects a structured result from a secondary
transform.

The workflow and its transforms are authored together but never run
together. The workflow body runs in the control plane and each transform is
evaluated from its own bundled artifact; nothing is serialized with `Function.prototype.toString()` and no
closure crosses the boundary. Instead, a build step splits the module before
it runs (`crates/orchestration/src/build/split.rs`):

1. The module is parsed with oxc. Every
   `jssg({ ... })` call (an object literal with a string-literal `name` and a
   `transform` that is a method, a function or arrow expression, or an
   imported binding) is located by position.
2. For each transform, a virtual entry `export default <transform>` plus
   the import declarations it references is bundled with rolldown into one
   self-contained ES module: helpers imported from other modules (and their
   imports, including packages) are inlined; `codemod:*` modules and
   built-ins that the runtime provides stay as imports; types are erased.
3. The artifact's identity is its `name` and the SHA-256 of the bundled
   source. The module is rewritten so `transform` is that `{ name, hash }`
   reference, and the rewritten module is what the bundle holds. Module comments
   in the bundle are relative paths, so the hash is the same on every
   checkout and changes whenever the transform or a bundled helper changes.

Supported subset, checked at build time with a source position rather than
failing at run time:

- A transform may use its own parameters and locals, globals, and bindings
  the workflow module imports from other modules.
- It may not use anything else declared in the workflow module: a top-level
  `const`, `let`, `function`, or `class` is an unsupported lexical capture.
  Move such helpers into a module and import them; dynamic values enter
  through invocation input, never through capture.
- It may not use bindings imported from `@codemod.com/orchestration`; the
  orchestration API is not part of an artifact.
- `name` must be a string literal and the argument an object literal.
  Generators are rejected. `jssg(options)` with a variable is not extracted.

A transform function that reaches `jssg()` at runtime means the module was
loaded without the build step; the call throws with that explanation.
The workflow bundler (`crates/orchestration/src/build/bundle.rs`, used by
`codemod workflow run -w`) applies the split to every module in the workflow's
graph that calls `jssg(`, including definitions in imported files and packages
under `node_modules`, and carries the artifacts it collected in the bundle.
`build_module(source, file)` and `build_file(file)` do the split without
running anything. Artifacts are
executor-side data (the bundle holds them for `quickjs`); a JSSG command whose
artifact the executor does not hold fails in phase `artifact` before anything
runs.

The build step is Rust (oxc parses and rewrites, rolldown bundles) compiled
into the `codemod` binary, so building a workflow needs neither Node nor an
npm build dependency.

### The static selector

`selector` is optional ast-grep rule data, `{ rule, constraints?, utils? }`,
typed for the definition's language. The runtime evaluates it natively
(`crates/orchestration/src/jssg_batch.rs`), before the file reaches JavaScript, on every selected file; files without a
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
4. The runtime's batch loop (`src/execution/jssg-batch.ts`, in-process in
   the QuickJS host) receives the artifact source and the whole batch. It
   verifies the source against the recorded hash, evaluates it under a
   virtual module name (once per run), builds one semantic provider, indexes
   the whole batch in workspace mode, skips the files the selector does not
   match, and transforms the rest from the content it was given. Snapshot
   semantics: no transform sees another transform's edits, unlike the
   workflow engine, which writes each file before moving to the next.
   `write()` and `jssgTransform()` are staged, never written.
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

A transform that writes through a filesystem module (the curated `fs` in
`quickjs`) bypasses the batch result and is not tracked.

## Agent and assessment steps

```ts
import { agent, assessment, dynamic } from "@codemod.com/orchestration";

// File-oriented assessment. Each matched file is assessed individually by the
// decision model the environment names. The model receives the file's path,
// content, and optional validated input as state automatically — `ask` defines
// QUESTIONS ONLY.
const assessSources = assessment({
  name: "assess-sources",
  include: ["src/**/*.ts"],
  exclude: ["**/*.d.ts", "**/*.generated.ts"],
  // model: "jev-1.13.0", // optional pin; default DECISION_MODEL, else the provider's default
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
      type: "yesno" as const,
      instructions: "Is there enough evidence to automate this file's migration?",
      criteria: {
        yes: "The migration can be attempted and verified automatically",
        no: "A person should inspect the file before any write",
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
  // Any answer may be { type: "refused" }, so narrow on its type first.
  const anyManual = results.some(({ assessment: { answers } }) => {
    const route = answers.route;
    return route.type === "refused" || route.choice === "manual" || route.confidence < 0.75;
  });
  if (!anyManual) {
    await finish({ input: change });
  }
  return {
    files: results.length,
    manual: anyManual,
    routes: results.map(({ file, assessment: { answers: { route } } }) => ({
      file,
      route: route.type === "choice" ? route.choice : "refused",
    })),
  };
});
```

`assessment({ name, include, exclude?, ask, input?, model? })` is a
file-oriented, read-only judgment by a decision model. It selects files using
JSSG-style include/exclude globs, reads each file's content, and asks the
configured decision model once per file with bounded concurrency. The
questions and answers are this package's own shapes (`src/core/assessment.ts`);
a provider adapter translates them to the provider's API, and the environment
picks the provider and model:

- **File selection.** `include` (required, non-empty) and `exclude` (optional)
  are glob patterns relative to the `--target` root. They use the same JSSG-style
  walker (`src/execution/files.ts`) with gitignore semantics: hidden files
  visited, symlinks skipped and never followed, `.ignore`/`.gitignore`/
  `.git/info/exclude` and global git excludes honored, include/exclude globs
  taking precedence over ignore files. Order is deterministic component-wise
  byte order. Each selected file is read as strict UTF-8; a file that vanished
  after selection is skipped, and invalid UTF-8 or another read error fails
  the command.
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
  Each file's `assessment` contains `{ provider, model, calibrated, answers,
  usage }`, typed from the questions. Each file's response is validated
  against the exact questions resolved for that file.
- **Concurrency.** The assessment command holds one scheduler permit for
  the whole batch; inside it, up to `fileConcurrency` files (default 4,
  clamped to 1–32) are requested at a time.
- **Command IDs.** A single-file result uses the assessment name as the command
  id. Multi-file results use `${name}:${file.path}` per file.
- **Replay** uses the recorded operation and result without rereading files.
  The command records `state`, `questions`, `provider`, and `model` as content.
  `provider` and `model` are resolved from the environment when the command is
  created (a pinned `model` option wins), so changing any of them, including
  `DECISION_PROVIDER` or `DECISION_MODEL`, replays as `changed`.
- **Repository-level assessment is intentionally not supported.** To assess a
  summary (build output, CI log, aggregated metrics), have a preceding shell
  or dynamic step materialize a single summary file and assess that file.
- `questions` are named. `choice` picks one of the `criteria` keys (at least
  two; values are descriptions or `null`); `score` rates against ordered
  `criteria` levels (at least two); `yesno` is a yes/no question with optional
  `criteria: { yes, no }`. Instructions and descriptions may be text or
  JSON. Malformed questions throw when `toOperation` is called (i.e. when the
  command is issued), since questions may depend on runtime input. Question
  ids and choice options may not be `__proto__`, `constructor`, or
  `prototype`.
- The per-file result is `{ provider, model, calibrated, answers, usage }`,
  typed from the questions: a choice answer has `choice` (one of the declared
  options), `probabilities` per option, and `confidence`; a score answer has
  `score` (may fall between levels), `legend`, `probabilities` per level, and
  `confidence`; a yesno answer has `yes`, the probability of yes. Any answer
  may instead be `{ type: "refused" }` when the model declined, so workflow
  code narrows on `answer.type`. `provider` is
  the adapter that asked, `model` is the versioned model that answered (e.g.
  `jev-1.13.0` for `jev-latest`), `calibrated` says whether the provider
  documents its probabilities and confidence as calibrated (configurable,
  below), and `usage` is
  `{ inputTokens?, outputTokens? }`, each count present only when the API
  reported it. Answers are validated against the concrete resolved questions
  both when the API responds and when a result is decoded (replay included);
  scores, probabilities, and confidence may exceed their range by
  floating-point noise (at most `1e-6`) and are kept unmodified.
- It decides nothing. Thresholds, fallbacks, and routing are workflow code.
- The runtime runs it in-process: one JSON `POST` per file to the decision API
  (`executeAssessmentOperation` in `src/execution/assessment.ts`, portable,
  reading files through the `Host`), encoded and decoded by the provider's
  adapter in `src/execution/decision/`. Configuration is environment-only
  (`src/core/decision.ts`, resolved by `decisionModel` and `decisionSettings`);
  workflow code never names an endpoint or a key. `DECISION_PROVIDER` is
  `systemone` (default) or `openai`; `DECISION_BASE_URL`, `DECISION_PATH`,
  and `DECISION_MODEL` default per provider; `DECISION_API_KEY` is the key;
  `DECISION_AUTH_HEADER` is `authorization` (default, sends `Bearer <key>`)
  or another header that carries the raw key, such as `api-key`. `systemone`
  still reads `TYPESAFE_BASE_URL`, `TYPESAFE_DEFAULT_MODEL`, and
  `TYPESAFE_API_KEY` when the `DECISION_*` value is unset, and `openai` reads
  `OPENAI_API_KEY`. Blank values count as unset. An unknown
  `DECISION_PROVIDER` throws when the command is created.
- `systemone` (`decision/systemone.ts`) speaks TypeSafe's System One API
  (`POST /v1/systemone`, <https://docs.typesafe.ai/api>; default root
  `https://api.typesafe.ai`, model `jev-latest`): TypeSafe's Jev, Liquid AI
  d1, open servers that speak it, and Microsoft Decision on Azure AI Foundry
  (`DECISION_PATH=/providers/microsoft/v1/systemone`,
  `DECISION_AUTH_HEADER=api-key`, the deployment name as `DECISION_MODEL`).
  On that wire a yes/no question is `noul` with `{ true, false }` criteria;
  the adapter translates. `openai` (`decision/openai.ts`) speaks OpenAI's
  Decisions API (`POST /v1/decisions`; default root `https://api.openai.com`,
  model `gpt-6-luna`): questions go as an array of `{ name, type, instructions }`
  with type `predicate`, `choice`, or `score`, choice options as `choices`
  (`{ value, description }`), score levels as `levels` (`{ label, description }`,
  2 to 10), at most 400 questions per request. It takes text only, so JSON
  instructions are stringified and yes/no criteria are appended to the
  instructions; JSON state is sent as an `input_json` user message and text
  state as the `input` string. A refusal becomes `{ type: "refused" }`, and
  usage reports input tokens only.
- `calibrated` is `true` for TypeSafe and OpenAI, and `false` when
  `DECISION_PATH` is under `/providers/microsoft/`: Microsoft's docs describe
  its probabilities as useful for relative ordering and thresholds and do not
  claim calibration. `DECISION_CALIBRATED=true|false` overrides either
  default; any other value fails the command as a config error.
- In `quickjs` there is no global `fetch`: the executor gets an adapter
  (`src/host/quickjs/fetch.ts`) over the host function `httpRequest`, which
  only allows URLs under the decision API root, resolved as `decision.ts`
  resolves it (`DECISION_BASE_URL`, else `TYPESAFE_BASE_URL` for `systemone`,
  else the provider's default; same scheme, host, and port, a path under the
  root's path, no redirects followed), so workflow and transform code get no
  general network access. (Called from Node, the executor uses the global
  `fetch` unless `AssessmentExecutorOptions.fetch` is given.)
  `AssessmentExecutorOptions` (`{ env, fetch, timeoutMs, retry,
  fileConcurrency }`, all optional, the first argument of
  `executeAssessmentOperation`) carries no key or endpoint: `env` (default the
  host environment) is where the `DECISION_*` settings are read. Assessments
  never write to the terminal.
- Retries and timeouts are the transport's (`src/execution/decision/transport.ts`,
  the policy the TypeSafe SDK used before it was dropped); nothing else
  retries an assessment, so there is one retry layer. By default that is up
  to 2 retries for HTTP 408, 409, 429, and 500-599, connection failures, and
  attempt timeouts, with exponential backoff (500 ms to 5 s, 25% jitter) or
  the server's `retry-after-ms` / `Retry-After` when it is at most 60 s.
  `retry` overrides parts of the `RetryPolicy` (`{ maxRetries,
  initialDelayMs, maxDelayMs, jitter }`; `{ maxRetries: 0 }` disables
  retries). `timeoutMs` (default 30 s) is the per-attempt timeout; there is
  no total budget, so the worst case is `(maxRetries + 1) * timeoutMs` plus
  backoff waits. The run's `AbortSignal` cancels the in-flight attempt and
  any backoff wait. The evaluation is read-only, so repeating it is safe.
- Settings are checked before any request: `timeoutMs` between 1 and Node's
  timer limit (about 24.8 days), `retry.maxRetries` at most
  `MAX_ASSESSMENT_RETRIES` (10), an effective base URL that is an absolute
  `http(s)` URL without query, fragment, or user, an API key, and a `fetch`.
  Any of these is a `failed` completion with `phase: "config"` and no
  request, never a thrown error.
- Failures are `failed` with `error.details`, whose `phase` is `config` (the
  checks above), `request` (a connection error, or a timeout with
  `timedOut: true`, both `retryable`; or questions the provider cannot take,
  such as more than 10 score levels for `openai`), `http` (a non-2xx
  response, with `status`, `retryable`, `requestId` from `x-request-id` or
  `apim-request-id`, `retryAfterMs` when the server sent a usable one, and the
  parsed `body`, cut to 2,000 characters as text when longer), `response` (a
  2xx body that is not JSON or does not answer the questions: it is decoded
  and validated here), or `internal` (anything else). For `request` and
  `http`, `retryable: true` means the retries were exhausted. File-oriented
  assessments add `select` (file selection failed), and `utf8` and `read`
  (with the `file`). An abort is `cancelled`. Nothing is ever `unknown`: an
  assessment cannot have changed the repository.

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
  setting throws when the step is defined; the agent runner rejects the same
  requests as parse errors. Top-level `tools`/`maxSteps` are not accepted.
- The runtime runs an agent out of process, in one `codemod-agent` runner
  per step (`crates/orchestration-agent`, launched by the portable
  `src/execution/agent.ts` through the host's `spawn`), because its tools
  (`bash`, file tools, MCP servers, the `claude`/`codex` CLIs) run in the
  process working directory and inherit its environment. The runner starts in
  `--target`, so the backend may change files. The request goes to it as one
  line of JSON on stdin and the completion comes back as JSON on stdout (see
  `PROTOCOL.md`). Validated input is appended to the prompt as JSON; the
  prompt goes to external CLIs on stdin, never in argv.
- The runner is found through `CODEMOD_AGENT_BIN`; when it is unset,
  `codemod workflow run -w` sets it to the `codemod-agent` next to its own
  binary, if there is one. Build it with `pnpm build:agent`
  (`cargo build -p orchestration-agent`), which puts it next to a `codemod`
  built in the same target directory. It is checked only when an agent step
  runs: a missing runner is `failed` with `phase: "config"`.
- The result is `{ text }` for every backend: the builtin agent's final
  response, Claude Code's `result`, or Codex's last message. Progress and
  event streams are not kept. With an `output` schema the operation carries
  `responseFormat: "json"`, the runner appends an instruction to reply with a
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
  Rig at all. They never receive `LLM_API_KEY`. The runner checks login state
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
    directory the runner creates for the run; `config.toml` (MCP servers,
    profiles, trust overrides, `notify`) and execpolicy `.rules` files are not
    loaded, while auth still comes from `CODEX_HOME`. The final text is the
    last `agent_message` in the `--json` event stream, scanned line by line
    with lines over 16 MiB skipped; no output file path is given to Codex.
  - Both CLIs start with a private `TMPDIR` and without any variable whose
    name looks like a credential or provider setting (`*_API_KEY`, `*TOKEN*`,
    `*SECRET*`, `AUTH`, `ANTHROPIC_*`, `OPENAI_*`, `CODEX_*` except
    `CODEX_HOME`, `AWS_*`, ...), so their tools cannot inherit one.
    An explicit runtime `env` (`RuntimeSettings.env`, `AgentExecutionOptions.env`)
    containing such a name is refused for external backends as a `config`
    failure before anything starts.
- Host wall-clock limit: `executeAgent({ ..., externalAgentTimeoutMs }, ...)`,
  default `DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS` (30 minutes), applies to
  `claude-code` and `codex` runners. When it passes, the runner's process tree
  is killed as on abort and the command is `unknown` with
  `details: { phase: "execute", timedOut: true, repositoryMayBeModified: true }`.
  It is host configuration, not part of history; an invalid value is a
  `config` failure. Inside the runner, a CLI that exits while a descendant
  still holds its stdout is read for at most 2 more seconds.
- Outcomes. `failed` with `details: { phase: "config",
  repositoryMayBeModified: false }` means the agent never started: no runner
  binary; no `LLM_API_KEY` or repeated tools or invalid `maxSteps` (builtin);
  the CLI missing from PATH, not logged in, its login check failing or timing
  out (30 s), the process failing to spawn, (codex) no git repository, a
  credential-looking explicit `env` name, or an invalid
  `externalAgentTimeoutMs`. `failed` with `phase: "execute",
  repositoryMayBeModified: true` means the agent started and did not produce a
  successful final response (a model or tool error, a non-zero CLI exit, an
  error result, a missing final message), so files may have changed. `unknown`
  (`repositoryMayBeModified: true`) means the host lost track of a started
  runner: it failed to start, was aborted, killed, or timed out, crashed, no completion on
  stdout, or a completion that is invalid or names another command.
  `cancelled` only happens for an abort before the runner was spawned.

Security model of `agent` (a trusted-local prototype, not a sandbox):

- Environment: the runner does not inherit the host environment. It gets
  `AGENT_ENV_ALLOWLIST` (process basics such as `PATH`, `HOME`, `TMPDIR`,
  `LANG`, `LC_*`, the Windows equivalents, proxy and CA variables,
  `CLAUDE_CONFIG_DIR` and `CODEX_HOME` so a non-default CLI home is found, and
  for `builtin` only `LLM_PROVIDER`, `LLM_MODEL`, `LLM_BASE_URL`) plus the
  runtime's explicit `env`, which wins (names compared case-insensitively on
  Windows). External backends get no `LLM_*` or provider variables and no
  stdin secret. Decision API keys (`DECISION_API_KEY`, `TYPESAFE_API_KEY`),
  `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, cloud, CI, and registry credentials
  stay out unless passed explicitly.
- The API key: the launcher (`agentLaunch`) never puts `LLM_API_KEY` in the
  runner's launch environment, so `ps eww` or `/proc/<pid>/environ` of the
  runner does not show it. It sends the request line and then
  `{"LLM_API_KEY": ...}` on the runner's stdin with
  `CODEMOD_AGENT_SECRETS=stdin`; the key never enters the request or history.
  The runner reads it before any thread starts and removes `LLM_API_KEY` and
  the marker from its environment, so tool processes do not inherit them.
  When the runner is launched another way with `LLM_API_KEY` in its
  environment, removal only affects inheritance: the launch environment stays
  readable through `/proc` and `ps` for the process's lifetime. Either way the
  key is in the runner's memory, and the host's own environment (with whatever
  the user exported) is readable by same-user processes. Opting into `bash` or
  `mcp_tool` therefore lets the model read anything the user can, including
  other processes' environments where the OS permits.
- The response channel: before any thread or child exists, the runner moves
  stdout to a private close-on-exec descriptor and points fds 0 and 1 at
  `/dev/null`, so tool processes never see the request (or the key) and
  cannot write the completion.
- Builtin tools are not confined. The file tools accept any absolute path the
  user can read or write, not just `--target`, and the editor's directory view
  shells out to `find`. `bash` and `mcp_tool` run arbitrary commands with
  network access. There is no filesystem, process, or network sandbox:
  `quickjs` sandboxes the workflow and its transforms, not the agent runner.
- External harness confinement is whatever the CLI enforces: Claude Code's
  `--restricted` file-tool confinement and permission rules (an opted-in
  `Bash` runs commands as the user, with network access), and Codex's OS
  sandbox (`read-only` or `workspace-write`). The runner cannot verify either,
  and a CLI update can change them. Both CLIs read the user's own login state
  and may load `CLAUDE.md`/`AGENTS.md` from the repository, which steers the
  agent like any prompt text. `--restricted` ignores project settings, so a
  repository `.claude/settings.json` `apiKeyHelper` or hook is not loaded
  (the claude-code live test asserts both left no trace); Codex ignores user
  and project `config.toml` under `--ignore-user-config` (the codex live test
  asserts a project `notify` command did not run).
- Cancellation kills the runner and, best effort, its process tree. On POSIX
  the runner leads its own process group: the group is stopped, one `ps`
  snapshot (1 s timeout) finds descendants that left the group by parent id
  (such as the bash tool's session), those are stopped parents first and
  killed children first, then the group is killed. A stopped parent cannot
  reap an exited child, so its PID cannot be reused before the kill; the only
  reuse window is between the snapshot and stopping a parent outside the
  group. The QuickJS host's `kill` does this. A descendant that
  daemonized, or a sandbox that hides the process list (only the group is
  reached then), can leave processes behind.
- Host lifetime: runners are detached, so they do not receive the terminal's
  signals. `codemod workflow run -w` turns SIGINT, SIGTERM, and SIGHUP into an
  abort.
  Programs that embed `run()` with `executeAgent` must do the same: abort the
  run's `signal` on SIGINT, SIGTERM, and SIGHUP (or exit normally). If the
  host is killed with SIGKILL or crashes, nothing runs and runners and their
  agents can keep working orphaned. There is no parent-death channel: a runner
  that noticed its host dying could kill its own group, but not the
  descendants outside it without the same process listing, so it would be a
  partial cleanup and is deliberately not implemented.

## Running

For the trusted local prototype, run a TypeScript workflow with the Rust
`codemod` CLI:

```sh
cargo build -p codemod                    # the CLI; the runtime is built into it
cargo build -p orchestration-agent        # only for agent() steps: codemod-agent, next to codemod
target/debug/codemod workflow run -w ./path/to/workflow.ts --target ./repository
```

```text
codemod workflow run -w <workflow.ts|.mts> [--target <directory>] [--input <json>] [--history <file>]
```

- A `.ts` or `.mts` workflow runs on the `quickjs` runtime (see Runtime): the
  bundled workflow in one QuickJS context hosted by
  `crates/orchestration/src/host` inside the `codemod` process, with no
  Node at run time. It executes every operation kind and JSSG semantic
  analysis. The other `codemod workflow run` flags belong to YAML workflows and
  do not apply.
- `--target` (default: current directory) is where `shell` and `agent` run
  and the root JSSG targets are resolved beneath.
- `CODEMOD_AGENT_BIN` (default: the `codemod-agent` next to the `codemod`
  binary) locates the agent runner; it is checked only when an `agent` step
  runs.
- `--input <json>` supplies the root input when the workflow declares one.
- `--history <file>` resumes from the file when it exists and writes the
  run's history back to it.

The command prints the workflow's final value as JSON on stdout and errors on
stderr with exit code 1; workflow and transform console output also goes to
stderr.
`SIGINT`/`SIGTERM`/`SIGHUP` abort the run: child process trees (`shell`, agent
runners) are killed and the command in flight is recorded as `cancelled` with nothing written (an `agent`
already running is recorded as `unknown`, since it may have edited files).
`@codemod.com/orchestration` imports in a workflow resolve to `src/portable.ts` of the
runtime embedded in the binary (this package's `src` and the npm `ignore` package, copied
in when `codemod` is built); `CODEMOD_ORCHESTRATION_SRC=<path to src>` uses other sources
instead, for developing the runtime. A `node:*` import fails the build. The workflow
module and the sources it imports are expected to be ESM. This is a trusted-local runner: it makes no claim about untrusted
registry packages.

Startup is cached on disk under `$CODEMOD_WORKFLOW_CACHE` (default
`<tmpdir>/codemod-workflow-cache-<uid>`; a directory that is not private to the user
is not used): `quickjs/` bundles (so a warm run builds nothing) and `runtime/` the
embedded runtime's sources, written once per content hash. Entries are keyed by content
and checked against every file that went into them, so editing the workflow, a module it
imports, or a helper a transform bundles rebuilds; a resolution change that edits none of
those files (a new file shadowing an import, an edited `package.json` `exports` or
`tsconfig.json`) is not detected, so delete the directory.

Remaining work before Solid Migration Assistant can move onto this package:

- add the package to that repository and author its workflow with inline
  transforms and structured outputs; helpers it shares between transforms
  must be imported modules, not top-level declarations of the workflow file;
- ship a `codemod` release that includes the TypeScript runtime, and the
  `codemod-agent` runner next to it; today both must be built from this
  monorepo (or the runner pointed at with `CODEMOD_AGENT_BIN`);
- decide the cross-file edit policy Solid needs: today every transform sees
  the pre-command snapshot and two edits of one file in one command fail the
  command instead of chaining or merging;
- approvals and durable history remain unimplemented, so any Solid step that
  needs them stays in YAML;
- registry loading, and a review of the `quickjs` runtime as a boundary for
  untrusted workflow bodies, are still required before TypeScript workflows
  become an untrusted registry format; the build split already keeps
  transform artifacts independent of the workflow bundle so each can get its
  own bindings.

Prototype limitation of the build step: transform errors report positions in
the bundled artifact (named `<name>.jssg.js`), not the workflow source.

The programmatic API is `run()` with an `OperationExecutor` of your own for
control over history and settings (the test harness in `tests/support` runs
workflows that way with scripted executors). Running a whole workflow on the
QuickJS host is `codemod workflow run -w`, which resumes from and writes back a
history file with `--history <file>`.

## Runtime

```text
codemod workflow run -w wf.ts --target R
  Rust: cached_quickjs_bundle(wf.ts)      oxc + rolldown only on a cache miss
  orchestration::host::run(bundle, R)     in the same process
    Rust: one AsyncRuntime + one AsyncContext; transform built-ins (codemod:ast-grep over
          sg_core, ...) + LLRT globals; globalThis.__codemodQuickjsNative (fs, env, clock,
          spawn, jssg*)
    JS:   bundle entry installs the Host and async context, then
          run(workflow, { executor, runtime: QUICKJS_RUNTIME })
          jssg       -> executeJssgCommand -> batch loop (jssg-batch.ts) over BatchPlan:
                        jssgBegin -> [workspace: jssgIndex] -> per chunk jssgParse ->
                        per file transform(root, options) -> jssgFinish -> stage, commit
          shell      -> executeShell over Host.spawn (own process group, merged pipe)
          assessment -> executeAssessmentOperation (decision adapter, fetch over httpRequest)
          agent      -> executeAgent -> Host.spawn codemod-agent (key on stdin)
```

- **Bundle** (`crates/orchestration/src/build`): the entry, the portable closure,
  the workflow graph with each inline transform replaced by an `ArtifactRef`
  (so command identities are content hashes), and every artifact's source keyed
  by hash. `@codemod.com/orchestration` resolves to `src/portable.ts`; `node:*`
  fails the build; `codemod:*` stays external.
- **Host seam** (`src/core/host.ts`): synchronous `fs` with POSIX error codes,
  `path`, `os`, `env`, `spawn` (full explicit env, stdin, merged output, abort
  kills the process tree), `createAsyncContext`, clock, timers and `randomUUID`.
  No `src` module imports `node:*`; `tests/portable.test.ts` scans for it. The
  QuickJS host builds the `Host` from Rust primitives
  (`src/host/quickjs/host.ts`); the specs run on Node with a test-only Node host
  (`tests/support/node-host.ts`).
- **Transforms** are evaluated once per run per artifact hash, so module-level
  state is shared across files and commands of a run (unsupported). Batches of
  64 files or more run in chunks on the host's transform pool
  (`CODEMOD_JSSG_WORKERS` threads, else the core count capped at 4), each worker
  with its own context. Every batch rule (hash check, containment, selector
  skip, semantic provider, result conversion) is Rust, in
  `crates/orchestration/src/jssg_batch.rs`; the TypeScript loop owns only
  chunking, the workspace index pass, cancellation between files and turning
  failures into a completion.
- **File selection is native.** The host implements `Host.selectFiles` and
  `Host.fs.readSources` over `crates/orchestration/src/walker`, a port of
  `src/execution/files.ts` (the `ignore` npm package rule by rule, evaluated by
  `regress`), with subdirectories walked and files read on worker threads.
  `CODEMOD_NATIVE_SELECT=0` runs the TypeScript walker instead; same result,
  slower.
- **Source encoding.** Sources reach transforms verbatim, as the YAML engine
  hands them over: a BOM stays as U+FEFF, CRLF stays CRLF, a missing final
  newline stays missing, non-UTF-8 files are skipped. `jssg_parity.rs` compares
  the written bytes with the YAML engine's.
- **Async context.** Promise hooks are incomplete on the vendored quickjs-ng, so
  the bundle is built with async functions and async generators lowered to
  generators, and the entry patches `Promise.prototype.then` to carry the
  current frame into each continuation: the rule V8 uses for
  `AsyncLocalStorage`. Transform artifacts are not lowered.
- **Cancellation.** SIGINT, SIGTERM and SIGHUP abort the run; child process
  trees are killed. A JSSG batch yields to the event loop before each 64-file
  chunk, so a signal mid-batch cancels within one chunk with nothing written. A
  transform that never returns cannot be interrupted.
- **Capabilities** (`src/runtime/capabilities.ts`): `QUICKJS_RUNTIME` supports
  every operation kind and semantic analysis. A registered runtime that declares
  less is refused statically before anything runs, and operations issued inside
  `dynamic()` that it cannot execute complete as `failed` with
  `phase: "config"`.
- **Console:** workflow and transform `console.*` go to stderr; stdout is the
  result.

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
| `assessment` | 1 | HTTP requests only (up to `fileConcurrency` at once), no local work |
| `jssg` (no semantics or `"file"`) | 2 | parsing and transforming plus the whole selected file set in memory |
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
  for tests. `scheduler.stats()` reports
  `{ capacity, used, active, queued, peakActive, peakUsed, paused }`, and the
  run's `EventSink` receives `scheduler.queued`, `scheduler.admitted`, and
  `scheduler.released`.

The host may pause admission: `scheduler.pause()` stops admitting from the
queue (new acquisitions queue even when capacity is free) while every admitted
operation runs to completion and returns its permit; `scheduler.resume()`
pumps the queue again from the head. Both are idempotent and report
`scheduler.paused` / `scheduler.resumed` to the scheduler's own `events` sink
(`new AdmissionScheduler({ events })`; the scheduler `run()` creates by default
reports to the run's sink).

Cancellation splits by admission state. A command aborted while queued is
removed from the queue and completes `cancelled` without the executor ever
being called, so nothing is spawned for it. A command that was already admitted
keeps the existing behavior: its signal is aborted (a child process tree is
killed, a JSSG batch stops between files) and the completion is `cancelled` or
`unknown`. Permits are released on success, on a non-success
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
the run's runtime to the body through the host's async context
(`Host.createAsyncContext()`: then-propagation in the `quickjs` runtime, see
Runtime; `AsyncLocalStorage` on the test-only Node host), so commands awaited anywhere in the body's async continuations
issue to that run, two concurrent runs never see each other's runtime, and
nothing is stored on a process-global. A command awaited outside any run
rejects instead of running.

## Determinism warning

Running workflow code under the Node test harness is NOT a secure deterministic sandbox. Workflow code only
needs the runnables it imports, but nothing prevents it from reading `Date`,
`Math.random`, `fs`, or `process`. Determinism is validated after the fact by
replay comparison; if a workflow uses such inputs the replay will fail with
`NondeterminismError`. The `quickjs` runtime removes Node, `process`, and
general network access from the workflow body, but `Date` and `Math.random`
remain, so it does not make a workflow deterministic either.

## Migration seams

`OperationExecutor`, `HistoryStore`, `CommandGate`, and `EventSink` are small
interfaces with JSON-only inputs and outputs. Each in-memory implementation can
move to Rust one at a time without changing workflow source. `shell` runs
in-process over `Host.spawn`, local `jssg` has the TypeScript orchestrator
around the shared Rust batch rules (`crates/orchestration/src/jssg_batch.rs`), `agent` has a
process adapter over the `codemod-agent` runner (`codemod-ai` or an external
CLI), and `assessment` has in-process HTTP adapters per decision provider;
the executor is the QuickJS bundle's; the
harness can script any operation in unit tests (for `shell` and `agent` a
non-string value becomes JSON `stdout` / `text`).
`OperationExecutor.execute` takes an optional `AbortSignal`.

## Commands

```
pnpm install
pnpm --filter @codemod.com/orchestration test          # fast TS tests (fake agent runner, echo runtime)
pnpm --filter @codemod.com/orchestration typecheck
pnpm --filter @codemod.com/orchestration build:agent   # = cargo build -p orchestration-agent
cargo test --release -p codemod --test typescript_workflow --test jssg_parity   # end to end
cargo test -p orchestration                            # the jssg() split, bundles, cache, batch rules, walker
cargo test -p orchestration-agent                      # agent protocol fixtures, binary, external CLIs
```

The vitest suite covers the portable runtime. The end-to-end specs run
`codemod workflow run -w` on the fixtures in `tests/fixtures`:
`typescript_workflow.rs` the whole engine, `jssg_parity.rs` JSSG edge cases on
`quickjs`, with the encoding case compared byte for byte with the YAML engine.

Live tests call real services, cost money or subscription quota, and are
opt-in, one flag per backend. Each live agent test uses a temporary git
repository, a tiny deterministic task (read `VERSION`, write `RELEASE.txt`,
answer with JSON), the backend's default settings (no shell tool, no bypass
flags), and a 5-minute timeout. Ordinary `test`
runs report them as skipped and make no request, even when credentials are
set; opting in without the required credentials fails instead of skipping.

```
# assessment() eval against each named provider (other DECISION_* settings optional;
# with both providers, set TYPESAFE_API_KEY and OPENAI_API_KEY instead of one DECISION_API_KEY)
CODEMOD_LIVE_ASSESSMENT=systemone,openai DECISION_API_KEY=... pnpm --filter @codemod.com/orchestration test:live:assessment
#   = vitest run tests/assessment.live.test.ts

# builtin agent() through the real agent runner (LLM_PROVIDER, LLM_MODEL, LLM_BASE_URL, CODEMOD_AGENT_BIN optional)
LLM_API_KEY=... pnpm --filter @codemod.com/orchestration test:live:agent:builtin
#   = pnpm build:agent && CODEMOD_LIVE_AGENT_BUILTIN=1 vitest run tests/agent.builtin.live.test.ts

# claude-code agent() with the installed `claude` CLI and its existing login
pnpm --filter @codemod.com/orchestration test:live:agent:claude-code
#   = pnpm build:agent && CODEMOD_LIVE_AGENT_CLAUDE_CODE=1 vitest run tests/agent.claude-code.live.test.ts

# codex agent() with the installed `codex` CLI and its existing login
pnpm --filter @codemod.com/orchestration test:live:agent:codex
#   = pnpm build:agent && CODEMOD_LIVE_AGENT_CODEX=1 vitest run tests/agent.codex.live.test.ts
```

The live assessment test calls the decision APIs with Node's global `fetch`,
which ignores `HTTPS_PROXY`; behind a proxy add `NODE_USE_ENV_PROXY=1`
(Node 24) to the assessment command.

The vitest suite never builds or uses the `codemod` CLI; the cargo specs above
do.

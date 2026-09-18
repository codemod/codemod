# TypeScript Orchestration Proposal

> Status: prototype for team review. This is not a shipped API or migration commitment.

## Problem

Most Codemod workflows are small, but every package uses the same YAML graph,
step, state, and scheduling model:

- 613 current packages contained 616 workflows.
- The median workflow had one node and one step.
- 379 packages (61.8%) had exactly one workflow, node, and step.
- JSSG appeared in 530 packages, while only 12 used workflow state.

Simple transforms should need less setup. Complex workflows still need branches,
parallel work, agents, approvals, recovery, and repository-wide coordination.

## What Changes

Today, a package describes nodes, steps, dependencies, and action-specific
settings in YAML. The Rust engine parses that file, builds a graph, schedules
each step, and sends work to a runner. Even a package with one JSSG transform
must use this workflow shape.

This proposal changes the package-facing definition, not the whole execution
stack. It does not move shell or JSSG execution into Node, and it does not yet
convert existing YAML packages:

```text
Today:     workflow.yaml -> Rust graph and scheduler -> existing runners
Proposed:  TypeScript composition or workflow -> command history -> existing runners
```

A runnable is a typed description of one operation. `jssg()`, `shell()`,
`agent()`, and `assessment()` define runnables. Invoking a runnable, `inspect()` or
`migrate({ input, target, id })`, creates a lazy command: plain data that a
static composition can hold, and that executes when a workflow awaits it. The
runtime executing the body is what an awaited command reaches.

| Current workflow concept | Proposed TypeScript form |
| --- | --- |
| `run` action | `shell()` |
| JSSG or AI action | `jssg()` or `agent()` |
| typed judgment on explicit data (no YAML equivalent) | `assessment()` |
| fixed sequence and data flow | `sequence()` |
| independent work | `parallel()` |
| condition based on an earlier result | normal `if` inside `dynamic()` |
| workflow-state handoff | operation return value passed as input |
| nested codemod | imported runnable used in static composition or a workflow |
| per-step `base_path`, `include`, `exclude` | `{ target }` on a JSSG invocation |
| `shard` step and `max_threads` | automatic scheduler behavior, no public helper |

The prototype defines all four operation shapes. The Rust bridge executes
`shell()`, inline JSSG transforms that a build step has bundled, and `agent()`
through the built-in agent; the TypeScript host executes `assessment()` against
the TypeSafe System One API through TypeSafe's official JavaScript SDK
(`@typesafe-ai/sdk`), which owns authentication, transport, and retries.

### Single JSSG leaf

The registry has 358 single AST-rule packages. In the proposed API, one can
export the operation directly, with the transform written next to it:

```ts
export default jssg({
  name: "remove-old-api",
  language: "typescript",
  include: ["**/*.{ts,tsx}"],
  selector: { rule: { pattern: "oldApi($ARG)" } },
  transform(root) {
    const edits = root.root().findAll({ rule: { pattern: "oldApi($ARG)" } }).map((n) => n.remove());
    return root.root().commitEdits(edits);
  },
});
```

`language`, `include`, and `exclude` are the definition's intrinsic applicability: what the
transform can process at all. They travel with the package and are not an
invocation choice; without `include`, the language's file extensions apply, as
in a YAML `js-ast-grep` step. `selector` is optional static rule data the
executor uses to skip files before any sandbox starts. `transform` is the
same single-function contract a standalone codemod exports; a build step
bundles it (with the modules it imports) into a standalone artifact whose
identity, the name plus a content hash, is what the recorded command carries,
so it is the same on every checkout and changes with the code. Where the
transform runs is chosen by the caller through the invocation's `target` (see
Targeting below). Scheduling controls such as the current YAML `max_threads`
do not belong on a JSSG definition.

The prototype currently runs operations inside static composition or `dynamic()`. Direct
leaf exports remain proposed; applicability fields, the static selector, and
inline transform execution are implemented.

## Proposal

Use typed operations as the common unit and provide three composable forms:

- `sequence(...)` is a static graph node that passes each output to the next
  stage and returns the final output. An invocation with explicit `input` ignores flowing input.
- `parallel(...)` is a static graph node that gives every member the same input
  and returns their outputs as a declaration-order tuple.
- `dynamic(...)` explicitly marks arbitrary TypeScript for dynamic control
  flow or data computation. Raw functions are not composition stages.

Both static nodes can be reimplemented with promises inside a workflow. Their
purpose is ahead-of-time topology: construction builds inspectable IR without
running workflow code, enables early type and shape checks, and lets a future
host schedule static regions without starting the workflow sandbox. Workflow
stages remain opaque in that IR until workflow bundles gain stable references.

There is no separate form for file selection. A JSSG invocation carries its own
`{ root, include, exclude }` target (see Targeting); nothing wraps composition or
other runnables.

Operations return plain JSON checked by schemas such as Zod. One result can be
passed directly to the next operation without workflow state. Intermediate
values are retained explicitly by a workflow stage when data flow is not linear.

### Nested bundle

Fifty-two parent packages contain 943 nested-codemod actions. Imported runnables
turn a fixed bundle into a normal sequence:

```ts
import renameApi from "@codemod/rename-api";
import updateImports from "@codemod/update-imports";

const format = shell({ name: "format", command: "npm run format" });

export default sequence(renameApi(), updateImports(), format());
```

The whole sequence is known before execution, like a fixed YAML graph. It can be
validated and sent to the existing scheduler later.

### Command and JSSG hybrid

Eleven current packages combine shell commands with AI or JSSG actions. When the next step
depends on command output, a workflow passes the typed result directly:

```ts
const inspect = shell({ name: "inspect", command: "node inspect.js", output: Project });
const migrate = jssg({
  name: "migrate",
  language: "tsx",
  include: ["**/*.{ts,tsx}"],
  input: Project,
  output: Summary,
  transform(root, options) {
    return { content: migrateFile(root, options.params.input), output: summarize(root) };
  },
});

export default dynamic(async () => {
  const project = await inspect();
  if (!project.needsMigration) return { migrated: 0 };
  return migrate({ input: project });
});
```

A command returned from the body is awaited by the runtime before the workflow
finalizes, so the last step needs no `await`. When the structure is fixed and
has no branch, the same handoff is `sequence(inspect(), migrate())`.

### Fixed parallel audit

Forty-four current workflows are parallel graphs with no dependencies. An
explicit group tells the scheduler that no member depends on another:

```ts
const todos = shell({ name: "todos", command: "rg -c TODO" });
const fixmes = shell({ name: "fixmes", command: "rg -c FIXME" });
const format = shell({ name: "format", command: "npm run format" });

export default sequence(parallel(todos(), fixmes()), format());
```

`parallel()` is an author assertion, not an inferred effect check. Runnables do
not carry `effects`, `readOnly`, or `idempotent` metadata. If `format` depends on
the checks, it stays outside the group as shown.

### Parallel transforms

Writable JSSG transforms can also be independent. The future scheduler can
pipeline them without increasing the global worker limit:

```ts
export default parallel(transformA(), transformB(), transformC());
```

`sequence(transformA(), transformB(), transformC())` places a global barrier after each
transform. In the parallel form, the runner may start `transformB` on one file
while `transformA` is still processing other files. It must lock a file before
reading it and hold that lock through its write, so only one transform performs
a read-transform-write cycle on that file at a time. Contention should resolve
in declaration order for reproducibility.

This only removes barriers; it does not create more workers or reduce total
work. It helps with small file sets and long-tail tasks, but adds little when
each transform already saturates every worker. A single scheduler must own the
worker pool to avoid oversubscription.

The author remains responsible for semantic independence. Transforms that rely
on repository-wide state, files created by an earlier transform, or a particular
order must use `sequence()`. File-level locking requires a runner such as the future
JSSG adapter that mediates file access. The prototype runs parallel members as
whole operations and provides no file locking; opaque shell commands cannot gain
that guarantee without isolation or a more constrained adapter.

How one transform's files are split across workers is the scheduler's business,
not the author's. `parallel()` says which transforms may overlap; it never says
how many workers to use or how to partition a file set. See Targeting for why
that partition is not a public helper.

### Bounded admission

The prototype implements the resource half of that split. `parallel()` is
eligibility; a scheduler owned by the run decides how much of it overlaps, so a
group of thirty-seven independent analyzers is authored as one group and no
workflow ever names a concurrency number.

The scheduler sits at the execution boundary rather than inside `parallel()`,
so sequences, procedural workflows, and dynamically built groups are all bounded by
one budget, and a future Rust executor inherits the same seam. It is a weighted
semaphore with a strict FIFO queue. Weights charge a JSSG batch more than an
`shell` because it reads and holds the whole selected file set, and charge a
workspace-semantic batch more again because the bridge also parses and indexes
that set as one workspace. Capacity is derived from `availableParallelism()`
and host memory and never exceeds the available CPU count. On a host whose
capacity is below an operation's nominal weight, that operation consumes the
whole capacity and runs alone. Strict FIFO trades some utilization for the
guarantee that a heavy command is never overtaken forever.

Two properties matter beyond the bound itself. Because the permit is held
around the executor call, and JSSG selection and reading happen inside it, a
queued command retains only its operation metadata: thirty-seven queued
analyzers do not hold thirty-seven repository snapshots. And because only
executed commands reach the executor, replay consumes no capacity at all.

Members begin in declaration order and outputs return in that order, whatever
order they are admitted or completed in. A nested branch may issue its next
stage before an earlier sibling does, so history marks commands under a static
parallel scope as concurrent and replay accepts either sibling issue order.
Bounded admission is a resource decision, not a write-safety mechanism;
parallel mutating members remain the author's independence assertion, and the
file-level scheduler above is still future work.

### Targeting

Many registry packages run one transform over part of a repository: a single
app in a monorepo, everything except generated code, or one package at a time.
Today each YAML JSSG step carries its own `base_path`, `include`, and
`exclude`. In the proposed API that selection is data on the JSSG invocation
itself. There is no generic `target()`, `scope()`, `within()`, or `shard()`
wrapper: only a JSSG adapter can enumerate and enforce a file set, so only a
JSSG invocation accepts one. `shell` runs a whole command, `agent` works on the
whole working directory, and `assessment` sees only explicit state; none of
them accepts target metadata.

A target is a small plain object that can be shared between invocations:

```ts
const web = { root: "apps/web", include: ["src/**"], exclude: ["**/generated/**"] };
```

`root` is a directory relative to the repository; `include` and `exclude` are
globs relative to `root`. Every field is optional, but an empty target is
rejected because it would look like a narrowing while selecting everything.

A static sequence gives the same target to two JSSG steps and none to `shell`:

```ts
import renameApi from "@codemod/rename-api";
import updateImports from "@codemod/update-imports";

const format = shell({ name: "format", command: "npm run format" });

export default sequence(renameApi({ target: web }), updateImports({ target: web }), format());
```

A dynamic workflow targets one invocation per discovered package, with an
explicit id because the same definition runs repeatedly:

```ts
export default dynamic(async () => {
  const project = await inspect();
  for (const pkg of project.packages) {
    await migrate({ input: project, target: { root: pkg.path }, id: `migrate:${pkg.name}` });
  }
});
```

The same ids let a static sequence target one definition twice:
`sequence(renameApi({ target: client, id: "rename-api:client" }), renameApi({ target: web, id: "rename-api:web" }))`.

A parallel group states independence and per-member targets in one place:

```ts
export default parallel(transformA({ target: web }), transformB({ target: web }));
```

The rules that make this coherent:

- **Definitions own applicability; invocations own the target.** A JSSG
  definition says which language and default file patterns it can handle. The
  invocation's target says which repository area this run should touch. The
  effective file set is their intersection: a target cannot widen a definition's
  applicability, and a definition cannot pin itself to one repository area.
- **A target is command content, not command identity.** It travels inside the
  JSSG operation on the wire and is part of the command record that replay
  compares, so the same id with a different target is a `changed` command, and
  adding a target to a previously untargeted command is also a change. It never
  creates an id. An invocation without `id` uses the runnable name, so one
  definition invoked twice in a composition or a run needs explicit ids; positional
  structural ids such as `rename-api#1` remain proposed.
- **Targets do not partition work.** A target says "these files", never "these
  files on this worker". Splitting the effective file set into physical shards,
  choosing worker counts, and holding per-file locks are automatic scheduler
  behavior. The current YAML `shard` step and `max_threads` have no
  author-facing replacement. Two targets over disjoint roots are a request for
  two selections, not for two workers; the scheduler may still run them on one.

What the prototype implements: every example above runs as written. A JSSG
invocation takes `{ input?, target?, id? }`; `shell`, `agent`, and `assessment` invocations take
`{ input?, id? }` and throw `TargetValidationError` when given a `target`, so a
target is never silently dropped. The target is validated and normalized when
the command is created (relative root without `..`, non-empty pattern lists, no
unknown fields), recorded in history, sent on the wire as `operation.target`,
and enforced by the TypeScript JSSG orchestrator: it enumerates the files
accepted by both the definition and the target (with the workflow engine's
walker semantics, pinned by a shared contract the engine walker also runs),
sends them in component-wise order as one batch to one Rust bridge process,
checks the returned edits for cross-file conflicts, commits all edits only
after every transform succeeded, and returns the per-file structured outputs
in that order.

What the prototype does not implement: `shell` still runs in the executor's
working directory with no file list, a transform's own `fs` access is limited
to the target root rather than to the enumerated set, and there is no
file-target scheduler, per-file locking, or parallel file execution.

### Dynamic analysis and finding collection

The Datadog pattern discovers monorepo projects at runtime. Azure Pipelines, ARM
managed identity, and accessibility workflows use locked state to collect
findings. Here each operation returns data, then one writer receives
the combined list:

```ts
const discover = shell({
  name: "discover",
  command: "node discover-packages.js",
  output: Packages,
});

const inspectPackage = shell({
  name: "inspect-package",
  input: Package,
  output: Report,
  command: 'node inspect-package.js "$PACKAGE_PATH"',
  env: (pkg) => ({ PACKAGE_PATH: pkg.path }),
});

const writeReport = jssg({
  name: "write-report",
  language: "typescript",
  include: ["REPORT.md"],
  input: Findings,
  output: Summary,
  transform: (root, options) => renderReport(root, options.params.input),
});

export default dynamic(async () => {
  const packages = await discover();
  const reports = await parallel(
    packages.map((pkg) => inspectPackage({ input: pkg, id: `inspect:${pkg.name}` })),
  );

  const findings = reports.flatMap((report) => report.findings);
  return writeReport({ input: findings });
});
```

The stable id ties each result to a package even if operations finish in a
different order: members start in declaration order, are recorded in that
order, and `reports` comes back in that order. The local `reports` array
replaces shared workflow state and a lock. The same `parallel()` helper
represents a fixed group when given runnables spread as arguments and a
dynamic group when given one array of commands created during a workflow.
Workflows do not reach for `Promise.all`: the group is the unit the scheduler
sees and the unit replay compares.

### AI follow-up from earlier results

AI appears in 68 current packages. Next.js to TanStack and accessibility
patterns feed earlier summaries or findings into prompts. That handoff becomes
normal data:

```ts
const findIssues = jssg({
  name: "find-issues",
  language: "tsx",
  output: Findings,
  transform: (root) => ({ content: null, output: collectIssues(root) }),
});

const writeGuide = agent({
  name: "write-guide",
  prompt: "Write a migration guide for these findings. Reply with the guide as JSON.",
  input: Findings,
  output: Guide,
});

export default dynamic(async () => {
  const findings = await findIssues();
  if (findings.length === 0) return null;
  return writeGuide({ input: findings });
});
```

`agent()` runs through the bridge, in the target directory, on the backend the
step names; the backend is part of the recorded command. The default,
`builtin`, is the Butterflow agent (`codemod-ai`, the Rig runtime behind YAML
`ai` steps) with a recorded tool list and step limit. `claude-code` and
`codex` hand the task to the installed, logged-in Claude Code or Codex CLI
instead. Those are local harnesses with their own agent loop, tools, and
subscription quota; they may load repository instructions and do not
exercise codemod-ai or Rig. Each backend exposes only settings it can
enforce (Claude Code a tool set, Codex a sandbox mode), and none of them runs
with a permission or sandbox bypass. The result is always the final response
as `{ text }`, parsed and validated as JSON when the runnable declares
`output`.

### Assessment before routing

Many AI steps in current packages exist only to decide what happens next: is
this package already migrated, is this diff safe to apply, which follow-up
fits. A generative agent is the wrong tool for that decision. `assessment()`
asks a System One model (TypeSafe's Jev by default) named, typed questions
about state the workflow passes explicitly, and returns probabilities.

The `ask` function resolves both the explicit model state and the assessment
questions from validated runtime input. This ensures dynamic criteria (choice
options derived from input, score levels computed at run time) cannot diverge
from the state the model evaluates. The concrete resolved questions drive
operation serialization, output validation, history, and replay:

```ts
const triage = assessment({
  name: "triage",
  input: Findings,
  ask: (findings) => ({
    state: { findings },
    questions: {
      action: {
        type: "choice" as const,
        instructions: "What should happen with these findings?",
        criteria: { autofix: "Mechanical and safe", review: "Needs a human", ignore: null },
      },
      breaking: { type: "noul" as const, instructions: "Could fixing these change public behavior?" },
    },
  }),
});

export default dynamic(async () => {
  const findings = await findIssues();
  const { answers } = await triage({ input: findings });
  if (answers.action.choice === "autofix" && answers.action.confidence > 0.8 && answers.breaking.noul < 0.2) {
    return fixIssues({ input: findings });
  }
  return writeGuide({ input: findings });
});
```

The assessment is read-only: no repository access, no tools, nothing but the
state it is given. It returns every answer's probabilities and confidence, the
model that answered, and token usage, and it decides nothing. The thresholds
and the routing stay in workflow code, where they are replayed and reviewed
like any other branch. The questions follow TypeSafe's primitives (`choice`,
`score`, `noul`) directly; Jev is the default model, not part of the
contract, and a command may pin another.

## Ownership

TypeScript owns the author-facing model, every piece of orchestration policy,
and the parts that need rapid iteration:

- runnable definitions and schema-based typing
- workflow and static-composition authoring, including the inline transform
- the build step that splits a workflow module into the trusted workflow and
  one bundled artifact per transform (TypeScript parser for extraction,
  esbuild for bundling, SHA-256 for identity)
- serializable static topology with opaque workflow boundaries
- the test harness
- prototype replay and in-memory history
- for JSSG: artifact lookup, repository traversal and language-extension
  defaults, definition and target intersection, deterministic ordering,
  reading sources, cross-file conflict validation, the transactional commit,
  typed output aggregation, cancellation, and failure classification

Rust owns only execution and the checks on its own side of the boundary. The
versioned JSON bridge calls the existing `butterflow_runners::DirectRunner`
for shell commands. For JSSG, one bridge process per command receives the
bundled transform source and the selected files with their contents, verifies
the source against the recorded hash, loads it from memory, builds one
semantic provider, evaluates the static selector natively, transforms every
eligible file through the existing QuickJS sandbox, validates every path it
receives or produces against the target root, and returns the edits and
outputs as plain JSON. It never reads author files, never enumerates the
repository, and never writes repository files on this path. It does not
implement planning, replay, or persistence, and it builds without the full
Codemod CLI.

```text
workflow.ts -> build step -> workflow module (Node) + transform artifacts (QuickJS)
TypeScript workflow -> replay gate -> BridgeExecutor
    shell -> bridge process -> DirectRunner
    jssg -> executeJssg (artifact, select, read) -> bridge process (one batch) -> executeJssg (validate, stage, commit)
```

The workflow and its transforms are split because they will run in
different sandboxes with different bindings: the workflow body sees the
orchestration runtime, a transform sees `codemod:ast-grep` and the curated
sandbox modules. The split happens before anything runs, on source text and
positions, never on function values; a transform may use its own code,
globals, and imported modules, and the build rejects any other capture from
the workflow module with a position. Dynamic values enter through invocation
input. History records the artifact's name and content hash; the executor
carries the source in the request context, outside history.

The split keeps the new authoring API and its policy easy to change while
reusing the execution behavior we already have. It also avoids rewriting shell
execution or the sandbox in Node. The boundary is plain JSON. For example,
TypeScript sends:

```json
{
  "protocolVersion": 8,
  "commandId": "format",
  "operation": { "kind": "shell", "command": "npm run format" }
}
```

Rust returns plain data:

```json
{
  "protocolVersion": 8,
  "commandId": "format",
  "status": "succeeded",
  "output": { "stdout": "formatted 12 files\n" }
}
```

The bridge exchanges request and completion files because non-CLI crates
must not write protocol messages to the terminal. `RUST_BRIDGE.md` documents
the protocol, the batch, the security model, and the transaction semantics.

The existing YAML engine is untouched: Butterflow keeps its graph,
scheduling, state, reporting, JSSG execution (including `getSelector`), and
filesystem mutation path. The shared changes are additive sandbox
primitives the bridge uses and the engine does not: the `stage_writes`
option, a loader-generic `execute_codemod_with_loader` behind the unchanged
`execute_codemod_with_quickjs`, and static selector helpers.

## Replay Model

History is an ordered execution record, not only a result cache:

```ts
const history = new MemoryHistoryStore();

const first = await run(migration, { executor, history });
// first.replayed === false; inspect and migrate executed

const second = await run(migration, { executor, history });
// second.replayed === true; recorded results were returned
```

Each issued command stores its command id, operation details, and completion.
The workflow output is stored last. Changing, moving, adding, or removing a
command causes `NondeterminismError` instead of mixing new code with old
history. Ordered workflow commands must retain their order. Commands marked as
concurrent by static `parallel()` may replay in another sibling-completion
order, but their identities and contents must still match. Repeated invocations
need explicit ids:

```ts
await lint({ id: "lint:client" });
await lint({ id: "lint:server" });
```

Workflow code must await every command it creates. A command that the body
created but never awaited, or awaited without waiting for its result, blocks
finalization: the runtime waits for running work to finish, then throws
instead of recording an output. This prevents command results from being
appended after finalization.

A command finds its run without a context argument. The Node prototype binds
the run's runtime to the body with `AsyncLocalStorage`, which follows the
body's async continuations, keeps concurrent runs apart, and is not a
process-global; a command awaited outside any run rejects. That binding is a
host concern, so in production it moves into the host: a restricted QuickJS
instance exposes the runtime to the workflow bundle it executes and nothing
else.

The prototype only detects nondeterminism after the fact. Workflow functions run
directly in Node and can access time, randomness, the filesystem, the network,
and process state. Durable or untrusted execution requires moving the same
workflow bundle into that restricted QuickJS host, which only exposes approved
APIs and the bound runtime.

## Prototype Scope

Included:

- typed, callable `shell`, `jssg`, `agent`, and `assessment` descriptors that create lazy commands
- JSSG invocation targets, validated when the command is created and carried on the wire
- static sequences and explicit parallel groups, fixed or built inside a workflow
- procedural workflows that await commands directly or accept flowing input
- append-only in-memory history and replay checks
- scripted TypeScript tests
- real `shell` calls through the existing Rust runner
- real `agent` tasks through the bridge on a recorded backend: the existing
  `codemod-ai` agent with a recorded tool list whose default has no shell, or
  the installed Claude Code or Codex CLI run non-interactively with their own
  login, restricted settings loading, credential-free tool environments, a
  host wall-clock limit, and no bypass flags; all with an allowlisted bridge
  environment, exchange files outside the target and any agent sandbox's
  writable roots, and best-effort process-tree cleanup on cancellation
- read-only `assessment` calls to the TypeSafe System One API from the host
  through the official `@typesafe-ai/sdk` client,
  with answers validated against the questions
- inline JSSG transforms split into bundled artifacts at build time and run
  through the existing sandbox as one Rust batch per command, with a static
  selector prefilter and workspace semantic analysis
- TypeScript-owned file selection with the engine's walker semantics,
  deterministic ordering, conflict checks, transactional commit, structured
  output aggregation, and failure classification
- cancellation of the operations in flight (bridge killed, nothing written) and
  refusal of the ones still queued (never started)
- weighted, host-derived bounded admission of parallel operations at the
  execution boundary, with operator/test capacity overrides and no
  author-facing concurrency knob
- an experimental trusted-local TypeScript workflow CLI
- a loopback-only dashboard behind that CLI's `--dashboard`: live command
  status, the static topology with dynamic stages left opaque, an operator
  admission pause/resume at the scheduler seam, and a host-layer session that
  restarts or re-runs the loaded configuration (abort, settle, then a fresh
  run with a new id, scheduler, and empty history) and keeps the last 20 runs
  of the process for read-only viewing (in-process only, not durable)

Not included:

- QuickJS workflow sandboxing and host-bound runtime (the prototype uses
  `AsyncLocalStorage`; the workflow side of the build split runs in Node)
- transform authoring beyond the supported subset: capturing the workflow
  module's own declarations, `options.matches` from the static selector,
  source-mapped sandbox errors
- agent sandboxing: agent file tools accept any absolute path, and opted-in
  `bash`/`mcp_tool` run arbitrary commands; approvals and streaming agent
  progress into events are also missing
- durable persistence, a production Rust scheduler, or adaptive telemetry that
  tunes capacity from observed load
- group transactions or cross-command merge semantics for parallel mutating
  members: independence is still the author's assertion
- a parallel file-job scheduler, per-file locks, worktrees, or merge semantics
  (every transform sees the pre-command snapshot; conflicting cross-file
  edits fail the command instead of chaining or merging)
- cross-file atomicity of the commit
- metrics, findings, artifacts, or human approval channels
- state-backed matrices, native shards, or delivery behavior
- a platform-neutral structured stdout/stderr result from `DirectRunner`

These omissions are explicit boundaries, not compatibility behavior to preserve.

## Migration Path

Four small interfaces separate execution, history, replay, and events. That lets
us move one part at a time:

1. Move command ID calculation and file-backed history into Rust.
2. Move replay comparisons and final output checks into Rust.
3. Let Rust execute operations directly, then add assessment and composition adapters. JSSG
   already runs through a narrow Rust batch; the orchestration around it
   (selection, conflict checks, commit) stays in TypeScript until the
   scheduler below exists.
4. Add one shared file-job scheduler that resolves each command's effective
   file set, shards it automatically, and holds per-file locks across each JSSG
   read-transform-write cycle.
5. Run workflow bundles in restricted QuickJS, with the runtime bound by the
   host instead of `AsyncLocalStorage`, before treating them as durable or
   untrusted.

Workflow bodies, runnable typing, schemas, composition authoring, and test ergonomics
should remain TypeScript. This keeps Rust focused on durable engine concerns
without freezing the authoring API too early.

## Evaluation

Before production adoption, validate the design against representative registry
packages: a single JSSG transform, a nested bundle, a command/JSSG hybrid, a
stateful repository analysis, and an agent workflow. The prototype should prove
that simple packages become smaller without hiding the controls needed by the
complex cases.

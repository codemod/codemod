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
Proposed:  TypeScript plan or workflow -> command history -> existing runners
```

A runnable is a typed description of one operation. `jssg()`, `exec()`, and
`ai()` define runnables. Invoking a runnable, `inspect()` or
`migrate({ input, target, id })`, creates a lazy command: plain data that a
plan can hold, and that executes when a workflow awaits it. The workflow body
takes no context argument; the runtime executing the body is what an awaited
command reaches.

| Current workflow concept | Proposed TypeScript form |
| --- | --- |
| `run` action | `exec()` |
| JSSG or AI action | `jssg()` or `ai()` |
| fixed sequence | `plan()` |
| fixed data flow | `pipe()` |
| independent work | `parallel()` |
| condition based on an earlier result | normal `if` inside `workflow()` |
| workflow-state handoff | operation return value passed as input |
| nested codemod | imported runnable used in a plan or workflow |
| per-step `base_path`, `include`, `exclude` | `{ target }` on a JSSG invocation |
| `shard` step and `max_threads` | automatic scheduler behavior, no public helper |

The prototype defines all three operation shapes. The Rust bridge executes
`exec()` and trusted local JSSG scripts; AI results remain scripted in tests.

### Single JSSG leaf

The registry has 358 single AST-rule packages. In the proposed API, one can
export the operation directly:

```ts
export default jssg({
  name: "remove-old-api",
  script: "scripts/remove-old-api.ts",
  language: "typescript",
  include: ["**/*.{ts,tsx}"],
});
```

`language`, `include`, and `exclude` are the definition's intrinsic applicability: what the
transform can process at all. They travel with the package and are not an
invocation choice; without `include`, the language's file extensions apply, as
in a YAML `js-ast-grep` step. `script` is relative to the package (the
workflow file's directory) so the recorded command identity is the same on
every checkout. Where the transform runs is chosen by the caller through the
invocation's `target` (see Targeting below). Scheduling controls such as the
current YAML `max_threads` do not belong on a JSSG definition.

The prototype currently runs operations inside `plan()` or `workflow()`. Direct
leaf exports remain proposed; applicability fields and local script execution
are implemented.

## Proposal

Use typed operations as the common unit and provide four composable forms:

- `plan(...)` runs fixed steps in order. It does not pass return values between
  them; repository changes are the usual handoff.
- `pipe(...)` creates fixed typed data flow from each output to the next input.
- `parallel(...)` declares that its members have no ordering dependency.
- `workflow(async () => ...)` uses normal TypeScript for dynamic control flow.

There is no separate form for file selection. A JSSG invocation carries its own
`{ root, include, exclude }` target (see Targeting); nothing wraps plans or
other runnables.

Operations return plain JSON checked by schemas such as Zod. One result can be
passed directly to the next operation without workflow state. `pipe()` is
proposed API work and is not implemented in the prototype; the other three
forms are.

### Nested bundle

Fifty-two parent packages contain 943 nested-codemod actions. Imported runnables
turn a fixed bundle into a normal plan:

```ts
import renameApi from "@codemod/rename-api";
import updateImports from "@codemod/update-imports";

const format = exec({ name: "format", command: "npm run format" });

export default plan(renameApi, updateImports, format);
```

The whole plan is known before execution, like a fixed YAML graph. It can be
validated and sent to the existing scheduler later.

### Command and JSSG hybrid

Eleven current packages combine shell commands with AI or JSSG actions. When the next step
depends on command output, a workflow passes the typed result directly:

```ts
const inspect = exec({ name: "inspect", command: "node inspect.js", output: Project });
const migrate = jssg({
  name: "migrate",
  script: "scripts/migrate.ts",
  language: "tsx",
  include: ["**/*.{ts,tsx}"],
  input: Project,
  output: Summary,
});

export default workflow(async () => {
  const project = await inspect();
  if (!project.needsMigration) return { migrated: 0 };
  return migrate({ input: project });
});
```

A command returned from the body is awaited by the runtime before the workflow
finalizes, so the last step needs no `await`. When the structure is fixed and
has no branch, the same handoff would be shorter as a typed pipeline,
`pipe(inspect, migrate)`, which remains proposed.

### Fixed parallel audit

Forty-four current workflows are parallel graphs with no dependencies. An
explicit group tells the scheduler that no member depends on another:

```ts
const todos = exec({ name: "todos", command: "rg -c TODO" });
const fixmes = exec({ name: "fixmes", command: "rg -c FIXME" });
const format = exec({ name: "format", command: "npm run format" });

export default plan(parallel(todos, fixmes), format);
```

`parallel()` is an author assertion, not an inferred effect check. Runnables do
not carry `effects`, `readOnly`, or `idempotent` metadata. If `format` depends on
the checks, it stays outside the group as shown.

### Parallel transforms

Writable JSSG transforms can also be independent. The future scheduler can
pipeline them without increasing the global worker limit:

```ts
export default parallel(transformA, transformB, transformC);
```

`plan(transformA, transformB, transformC)` places a global barrier after each
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
order must use `plan()`. File-level locking requires a runner such as the future
JSSG adapter that mediates file access. The prototype runs parallel members as
whole operations and provides no file locking; opaque shell commands cannot gain
that guarantee without isolation or a more constrained adapter.

How one transform's files are split across workers is the scheduler's business,
not the author's. `parallel()` says which transforms may overlap; it never says
how many workers to use or how to partition a file set. See Targeting for why
that partition is not a public helper.

### Targeting

Many registry packages run one transform over part of a repository: a single
app in a monorepo, everything except generated code, or one package at a time.
Today each YAML JSSG step carries its own `base_path`, `include`, and
`exclude`. In the proposed API that selection is data on the JSSG invocation
itself. There is no generic `target()`, `scope()`, `within()`, or `shard()`
wrapper: only a JSSG adapter can enumerate and enforce a file set, so only a
JSSG invocation accepts one. `exec` runs a whole command and `ai` has no file
set, and neither accepts target metadata.

A target is a small plain object that can be shared between invocations:

```ts
const web = { root: "apps/web", include: ["src/**"], exclude: ["**/generated/**"] };
```

`root` is a directory relative to the repository; `include` and `exclude` are
globs relative to `root`. Every field is optional, but an empty target is
rejected because it would look like a narrowing while selecting everything.

A static plan gives the same target to two JSSG steps and none to `exec`:

```ts
import renameApi from "@codemod/rename-api";
import updateImports from "@codemod/update-imports";

const format = exec({ name: "format", command: "npm run format" });

export default plan(renameApi({ target: web }), updateImports({ target: web }), format);
```

A dynamic workflow targets one invocation per discovered package, with an
explicit id because the same definition runs repeatedly:

```ts
export default workflow(async () => {
  const project = await inspect();
  for (const pkg of project.packages) {
    await migrate({ input: project, target: { root: pkg.path }, id: `migrate:${pkg.name}` });
  }
});
```

The same ids let a static plan target one definition twice:
`plan(renameApi({ target: client, id: "rename-api:client" }), renameApi({ target: web, id: "rename-api:web" }))`.

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
  definition invoked twice in a plan or a run needs explicit ids; positional
  structural ids such as `rename-api#1` remain proposed.
- **Targets do not partition work.** A target says "these files", never "these
  files on this worker". Splitting the effective file set into physical shards,
  choosing worker counts, and holding per-file locks are automatic scheduler
  behavior. The current YAML `shard` step and `max_threads` have no
  author-facing replacement. Two targets over disjoint roots are a request for
  two selections, not for two workers; the scheduler may still run them on one.

What the prototype implements: every example above runs as written. A JSSG
invocation takes `{ input?, target?, id? }`; `exec` and `ai` invocations take
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

What the prototype does not implement: `exec` still runs in the executor's
working directory with no file list, a transform's own `fs` access is limited
to the target root rather than to the enumerated set, and there is no
file-target scheduler, per-file locking, or parallel file execution.

### Dynamic analysis and finding collection

The Datadog pattern discovers monorepo projects at runtime. Azure Pipelines, ARM
managed identity, and accessibility workflows use locked state to collect
findings. Here each operation returns data, then one writer receives
the combined list:

```ts
const discover = exec({
  name: "discover",
  command: "node discover-packages.js",
  output: Packages,
});

const inspectPackage = exec({
  name: "inspect-package",
  input: Package,
  output: Report,
  command: 'node inspect-package.js "$PACKAGE_PATH"',
  env: (pkg) => ({ PACKAGE_PATH: pkg.path }),
});

const writeReport = jssg({
  name: "write-report",
  script: "scripts/write-report.ts",
  language: "typescript",
  input: Findings,
  output: Summary,
});

export default workflow(async () => {
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
  script: "scripts/find-issues.ts",
  language: "tsx",
  output: Findings,
});

const writeGuide = ai({
  name: "write-guide",
  prompt: "Write a migration guide for these findings",
  input: Findings,
  output: Guide,
});

export default workflow(async () => {
  const findings = await findIssues();
  if (findings.length === 0) return null;
  return writeGuide({ input: findings });
});
```

The AI adapter is future work; the TypeScript harness scripts this result today.

## Ownership

TypeScript owns the author-facing model, every piece of orchestration policy,
and the parts that need rapid iteration:

- runnable definitions and schema-based typing
- workflow and plan authoring
- serializable plan data
- the test harness
- prototype replay and in-memory history
- for JSSG: repository traversal and language-extension defaults, definition
  and target intersection, deterministic ordering, reading sources,
  cross-file conflict validation, the transactional commit, typed output
  aggregation, cancellation, and failure classification

Rust owns only execution and the checks on its own side of the boundary. The
versioned JSON bridge calls the existing `butterflow_runners::DirectRunner`
for shell commands. For JSSG, one bridge process per command receives the
selected files with their contents, loads the script and selector once, builds
one semantic provider, transforms every file through the existing QuickJS
sandbox, validates every path it receives or produces against the target
root, and returns the edits and outputs as plain JSON. It never enumerates
the repository and never writes repository files on this path. It does not
implement planning, replay, or persistence, and it builds without the full
Codemod CLI.

```text
TypeScript workflow -> replay gate -> BridgeExecutor
    exec -> bridge process -> DirectRunner
    jssg -> executeJssg (select, read) -> bridge process (one batch) -> executeJssg (validate, stage, commit)
```

The split keeps the new authoring API and its policy easy to change while
reusing the execution behavior we already have. It also avoids rewriting shell
execution or the sandbox in Node. The boundary is plain JSON. For example,
TypeScript sends:

```json
{
  "protocolVersion": 3,
  "commandId": "format",
  "operation": { "kind": "exec", "command": "npm run format" }
}
```

Rust returns plain data:

```json
{
  "protocolVersion": 3,
  "commandId": "format",
  "status": "succeeded",
  "output": { "stdout": "formatted 12 files\n" }
}
```

The bridge exchanges request and completion files because non-CLI crates
must not write protocol messages to the terminal. `RUST_BRIDGE.md` documents
the protocol, the batch, the security model, and the transaction semantics.

The existing YAML engine is untouched: Butterflow keeps its graph,
scheduling, state, reporting, JSSG execution, and filesystem mutation path.
The only shared change is a sandbox option (`stage_writes`) that the bridge
turns on and the engine leaves off.

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
history. Repeated invocations need explicit ids:

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

- typed, callable `exec`, `jssg`, and `ai` descriptors that create lazy commands
- JSSG invocation targets, validated when the command is created and carried on the wire
- static plans and explicit parallel groups, fixed or built inside a workflow
- procedural workflows that await commands directly, with no context argument
- append-only in-memory history and replay checks
- scripted TypeScript tests
- real `exec` calls through the existing Rust runner
- real local JSSG calls through the existing sandbox as one Rust batch per
  command, including workspace semantic analysis
- TypeScript-owned file selection with the engine's walker semantics,
  deterministic ordering, conflict checks, transactional commit, structured
  output aggregation, and failure classification
- cancellation of the operation in flight (bridge killed, nothing written)
- an experimental trusted-local TypeScript workflow CLI

Not included:

- QuickJS workflow sandboxing and host-bound runtime (the prototype uses `AsyncLocalStorage`)
- `pipe()`
- AI execution
- durable persistence or production scheduling
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
3. Let Rust execute operations directly, then add AI and Plan adapters. JSSG
   already runs through a narrow Rust batch; the orchestration around it
   (selection, conflict checks, commit) stays in TypeScript until the
   scheduler below exists.
4. Add one shared file-job scheduler that resolves each command's effective
   file set, shards it automatically, and holds per-file locks across each JSSG
   read-transform-write cycle.
5. Run workflow bundles in restricted QuickJS, with the runtime bound by the
   host instead of `AsyncLocalStorage`, before treating them as durable or
   untrusted.

Workflow bodies, runnable typing, schemas, plan authoring, and test ergonomics
should remain TypeScript. This keeps Rust focused on durable engine concerns
without freezing the authoring API too early.

## Evaluation

Before production adoption, validate the design against representative registry
packages: a single JSSG transform, a nested bundle, a command/JSSG hybrid, a
stateful repository analysis, and an agent workflow. The prototype should prove
that simple packages become smaller without hiding the controls needed by the
complex cases.

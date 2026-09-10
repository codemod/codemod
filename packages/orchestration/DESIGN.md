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
`ai()` define runnables; invoking a runnable in the proposed API creates a lazy
command controlled by the workflow runtime. The prototype still passes
descriptors to `w.run()` instead of making them callable.

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
| per-step `base_path`, `include`, `exclude` | `target()` around a runnable or plan |
| `shard` step and `max_threads` | automatic scheduler behavior, no public helper |

The prototype defines all three operation shapes, but the Rust bridge only runs
`exec()` today. JSSG and AI results are scripted in tests until adapters exist.

### Single JSSG leaf

The registry has 358 single AST-rule packages. In the proposed API, one can
export the operation directly:

```ts
export default jssg({
  name: "remove-old-api",
  package: "@codemod/remove-old-api",
  language: "typescript",
  files: ["**/*.{ts,tsx}"],
});
```

`language` and `files` are the definition's intrinsic applicability: what the
transform can process at all. They travel with the package and are not an
invocation choice. Where the transform runs is chosen by the caller with
`target()` (see Targeting below). Scheduling controls such as the current YAML
`max_threads` do not belong on a JSSG definition.

The prototype currently runs operations inside `plan()` or `workflow()`. Direct
leaf exports and the applicability fields are part of the proposed package
contract, not implemented wiring; the prototype `jssg()` accepts only `name`,
`package`, and schemas.

## Proposal

Use typed operations as the common unit and provide four composable forms:

- `plan(...)` runs fixed steps in order. It does not pass return values between
  them; repository changes are the usual handoff.
- `pipe(...)` creates fixed typed data flow from each output to the next input.
- `parallel(...)` declares that its members have no ordering dependency.
- `workflow(async () => ...)` uses normal TypeScript for dynamic control flow.

One modifier selects where composed work applies:

- `target({ root, include, exclude }, child)` narrows a runnable or plan to a
  repository area. It is not executable on its own and has no id.

Operations return plain JSON checked by schemas such as Zod. One result can be
passed directly to the next operation without workflow state. `pipe()`,
`target()`, and callable runnables are proposed API work and are not
implemented in the prototype.

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
  package: "@codemod/migrate",
  input: Project,
  output: Summary,
});

export default workflow(async () => {
  const project = await inspect();
  if (!project.needsMigration) return { migrated: 0 };
  return migrate(project);
});
```

When the structure is fixed and has no branch, the same handoff is shorter as a
typed pipeline: `pipe(inspect, migrate)`.

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
`exclude`. In the proposed API, that selection is one modifier, `target()`,
applied around the thing it should narrow.

Whole-plan targeting narrows every step at once:

```ts
import renameApi from "@codemod/rename-api";
import updateImports from "@codemod/update-imports";

const format = exec({ name: "format", command: "npm run format" });

export default target(
  { root: "apps/web", exclude: ["**/generated/**"] },
  plan(renameApi, updateImports, format),
);
```

Partial targeting narrows only some steps. Untargeted steps keep the enclosing
target, or the whole repository when there is none:

```ts
export default plan(
  target({ root: "packages/client" }, renameApi),
  target({ root: "packages/server", include: ["src/**/*.ts"] }, renameApi),
  format,
);
```

`target()` composes with the other forms. It can wrap a `parallel()` group so
the group's independence assertion and its file selection are stated once, and
a `pipe()` behaves the same way as a `plan()`:

```ts
export default target(
  { root: "apps/web" },
  plan(parallel(transformA, transformB), format),
);
```

Nested targets intersect. The inner `root` is resolved relative to the outer
one, and `include` and `exclude` lists are combined, so an inner target can only
narrow, never escape, the outer selection.

The rules that make this coherent:

- **A target is not a command and has no id.** A targeted runnable or plan
  remains executable, but the target never appears in history as its own
  command. Wrapping a runnable in `target()` does not change which command the
  scheduler records; it changes the content of that command.
- **Static identity comes from plan structure.** In the partial example above,
  `renameApi` appears twice and would clash on its runnable name. The plan gives
  each occurrence a positional id derived from structure, such as
  `rename-api#1` and `rename-api#2`, while `root`, `include`, and `exclude`
  become command content that replay compares. Changing a target on a step
  therefore reads as a `changed` command, not a new one.
- **Dynamic repeated calls still need explicit ids.** Inside `workflow()`,
  `target()` does not replace `{ id }`:

  ```ts
  export default workflow(async () => {
    const packages = await discover();
    await parallel(
      packages.map((pkg) =>
        target({ root: pkg.path }, renameApi)({ id: `rename:${pkg.name}` }),
      ),
    );
  });
  ```

- **Definitions own applicability; targets own invocation.** A JSSG definition
  says which language and default file patterns it can handle. A target says
  which repository area this invocation should touch. The effective file set is
  their intersection. A target cannot widen a definition's applicability, and a
  definition cannot pin itself to one repository area.
- **Targets do not partition work.** A target says "these files", never "these
  files on this worker". Splitting the effective file set into physical shards,
  choosing worker counts, and holding per-file locks are automatic scheduler
  behavior. There is no public `shard()`, `within()`, or `scope()` helper, and
  the current YAML `shard` step and `max_threads` have no author-facing
  replacement. Two targets over disjoint roots are a request for two selections,
  not a request for two workers; the scheduler may still run them on one.

The prototype does not implement `target()`. The Rust bridge executes whole
`exec` operations with no working directory or file list, there is no JSSG
adapter, and there is no file-target scheduler. Adding a `target()` that the
runtime ignored would let a plan claim to touch `apps/web` while rewriting the
whole repository, so it is left out rather than stubbed. The wire protocol will
need a target field on JSSG operations, and `exec` operations will need at least
a working directory, before it can be honored.

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
  package: "@codemod/write-report",
  input: Findings,
  output: Summary,
});

export default workflow(async () => {
  const packages = await discover();
  const reports = await parallel(
    packages.map((pkg) => inspectPackage(pkg, { id: `inspect:${pkg.name}` })),
  );

  const findings = reports.flatMap((report) => report.findings);
  return writeReport(findings);
});
```

The stable id ties each result to a package even if operations finish in a
different order. The local `reports` array replaces shared workflow state and a
lock. The same `parallel()` helper represents a fixed group when given runnable
definitions and a dynamic group when given commands created during a workflow.

### AI follow-up from earlier results

AI appears in 68 current packages. Next.js to TanStack and accessibility
patterns feed earlier summaries or findings into prompts. That handoff becomes
normal data:

```ts
const findIssues = jssg({
  name: "find-issues",
  package: "@codemod/find-issues",
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
  return writeGuide(findings);
});
```

The AI adapter is future work; the TypeScript harness scripts this result today.

## Ownership

TypeScript owns the author-facing model and the parts that need rapid iteration:

- runnable definitions and schema-based typing
- workflow and plan authoring
- serializable plan data
- the test harness
- prototype replay and in-memory history

Rust initially owns only operation execution. The versioned JSON bridge calls
the existing `butterflow_runners::DirectRunner`; it does not implement planning,
replay, persistence, or scheduling. A small file-based binary keeps this path
independent of the full Codemod CLI and avoids mixing protocol data with
terminal output.

```text
TypeScript workflow -> replay gate -> execution bridge -> DirectRunner
```

The split keeps the new authoring API easy to change while reusing the execution
behavior we already have. It also avoids rewriting shell execution in Node. The
boundary is plain JSON. For example, TypeScript sends:

```json
{
  "protocolVersion": 1,
  "commandId": "format",
  "operation": { "kind": "exec", "command": "npm run format" }
}
```

Rust returns plain data:

```json
{
  "protocolVersion": 1,
  "commandId": "format",
  "status": "succeeded",
  "output": { "stdout": "formatted 12 files\n" }
}
```

The bridge uses files because non-CLI crates must not write protocol messages to
the terminal. It builds without the full Codemod CLI.

## Replay Model

History is an ordered execution record, not only a result cache:

```ts
const history = new MemoryHistoryStore();

const first = await run(migration, { executor, history });
// first.replayed === false; inspect and migrate executed

const second = await run(migration, { executor, history });
// second.replayed === true; recorded results were returned
```

Each `w.run()` stores its command id, operation details, and completion. The
workflow output is stored last. Changing, moving, adding, or removing a command
causes `NondeterminismError` instead of mixing new code with old history.
Repeated calls need explicit ids:

```ts
await w.run(lint, { id: "lint:client" });
await w.run(lint, { id: "lint:server" });
```

Workflow code must await every `w.run()` call. The runtime waits for any missed
call to finish, but refuses to finalize that workflow run. This prevents command
results from being appended after finalization.

The prototype only detects nondeterminism after the fact. Workflow functions run
directly in Node and can access time, randomness, the filesystem, the network,
and process state. Durable or untrusted execution requires moving the same
workflow bundle into a restricted QuickJS host that only exposes approved APIs,
including `w.run()`.

## Prototype Scope

Included:

- typed `exec`, `jssg`, and `ai` descriptors
- static plans and explicit parallel groups
- procedural workflows
- append-only in-memory history and replay checks
- scripted TypeScript tests
- real `exec` calls through the existing Rust runner

Not included:

- QuickJS workflow sandboxing
- callable runnables and `pipe()`
- `target()`, JSSG applicability fields, and structural command ids
- JSSG and AI execution adapters
- durable persistence, cancellation, or production scheduling
- a shared file-job scheduler, per-file locks, worktrees, or merge semantics
- metrics, findings, artifacts, or human approval channels
- state-backed matrices, native shards, or delivery behavior
- a platform-neutral structured stdout/stderr result from `DirectRunner`

These omissions are explicit boundaries, not compatibility behavior to preserve.

## Migration Path

Four small interfaces separate execution, history, replay, and events. That lets
us move one part at a time:

1. Move command ID calculation and file-backed history into Rust.
2. Move replay comparisons and final output checks into Rust.
3. Let Rust execute operations directly, then add JSSG, AI, and Plan adapters.
   Carry `target()` root, include, and exclude on the wire as command content.
4. Add one shared file-job scheduler that resolves each command's effective
   file set, shards it automatically, and holds per-file locks across each JSSG
   read-transform-write cycle.
5. Run workflow bundles in restricted QuickJS before treating them as durable or
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

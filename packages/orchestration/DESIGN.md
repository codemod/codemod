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
parallel reads, agents, approvals, recovery, and repository-wide coordination.

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

A runnable is a typed description of one operation. Calling `jssg()`, `exec()`,
or `ai()` does not run anything. It creates a value that can be used by a plan
or passed to `w.run()`. The engine still decides when and where the operation
runs.

| Current workflow concept | Proposed TypeScript form |
| --- | --- |
| `run` action | `exec()` |
| JSSG or AI action | `jssg()` or `ai()` |
| fixed sequence or parallel readers | `plan()` |
| condition based on an earlier result | normal `if` inside `workflow()` |
| workflow-state handoff | operation return value passed as input |
| nested codemod | imported runnable used in a plan or workflow |

The prototype defines all three operation shapes, but the Rust bridge only runs
`exec()` today. JSSG and AI results are scripted in tests until adapters exist.

In the target API, a simple package can export the operation itself. There is
no need to add a workflow wrapper just to run one transform:

```ts
export default jssg({
  name: "remove-old-api",
  package: "@codemod/remove-old-api",
});
```

The prototype currently runs operations inside `plan()` or `workflow()`. Direct
leaf exports are part of the proposed package contract, not implemented wiring.

## Proposal

Use typed operations as the common unit and provide two orchestration modes:

- `plan(...)` creates a fixed, serializable graph at package build time.
- `workflow(async (w) => ...)` runs procedural TypeScript whose calls to
  `w.run(...)` are recorded and replayed.

Operations return plain JSON checked by schemas such as Zod. One result can be
passed directly to the next operation without workflow state.

A plan is the closest replacement for a fixed YAML graph. Its full shape is
known before execution, so it can be validated and sent to the existing
scheduler later:

```ts
const migrate = jssg({ name: "migrate", package: "@codemod/migrate" });
const format = exec({ name: "format", command: "npm run format" });

export default plan(migrate, format);
```

A workflow is for cases where the next operation depends on an earlier result.
The TypeScript function controls the branch, but operations only run through
`w.run()`, which lets the engine record them:

```ts
const inspect = exec({ name: "inspect", command: "node inspect.js", output: Project });
const migrate = jssg({
  name: "migrate",
  package: "@codemod/migrate",
  input: Project,
  output: Summary,
});

export default workflow(async (w) => {
  const project = await w.run(inspect);
  if (!project.needsMigration) return { migrated: 0 };
  return w.run(migrate, { input: project });
});
```

Parallel groups only accept read-only operations:

```ts
const todos = exec({ name: "todos", command: "rg -c TODO", readOnly: true });
const fixmes = exec({ name: "fixmes", command: "rg -c FIXME", readOnly: true });
const format = exec({ name: "format", command: "npm run format" });

export default plan(parallel(todos, fixmes), format);
```

`parallel(todos, format)` is rejected because `format` can write files. Writable
operations remain sequential until there is an isolation and merge model.

Dynamic parallel work uses a workflow because the list is only known after an
operation runs:

```ts
const discover = exec({
  name: "discover",
  command: "node discover-packages.js",
  output: Packages,
  readOnly: true,
});

const inspectPackage = exec({
  name: "inspect-package",
  input: Package,
  output: Report,
  readOnly: true,
  command: 'node inspect-package.js "$PACKAGE_PATH"',
  env: (pkg) => ({ PACKAGE_PATH: pkg.path }),
});

export default workflow(async (w) => {
  const packages = await w.run(discover);
  return Promise.all(
    packages.map((pkg) =>
      w.run(inspectPackage, {
        id: `inspect:${pkg.name}`,
        input: pkg,
      }),
    ),
  );
});
```

The stable id ties each result to a package even if operations finish in a
different order. The prototype runs this concurrently, but does not yet reject a
writable runnable inside `Promise.all`. Production dynamic parallelism must
enforce the same read-only rule as `parallel()`.

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
- static plans and read-only parallel validation
- procedural workflows
- append-only in-memory history and replay checks
- scripted TypeScript tests
- real `exec` calls through the existing Rust runner

Not included:

- QuickJS workflow sandboxing
- JSSG and AI execution adapters
- durable persistence, cancellation, or production scheduling
- mutable shared workflow state, locks, worktrees, or merge semantics
- metrics, findings, artifacts, or human approval channels
- a platform-neutral structured stdout/stderr result from `DirectRunner`

These omissions are explicit boundaries, not compatibility behavior to preserve.

## Migration Path

Four small interfaces separate execution, history, replay, and events. That lets
us move one part at a time:

1. Move command ID calculation and file-backed history into Rust.
2. Move replay comparisons and final output checks into Rust.
3. Let Rust execute operations directly, then add JSSG, AI, and Plan adapters.
4. Run workflow bundles in restricted QuickJS before treating them as durable or
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

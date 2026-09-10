# TypeScript Orchestration Proposal

> Status: prototype for team review. This is not a shipped API or migration commitment.

## Problem

The current workflow model makes simple codemods use the same YAML graph, state,
scheduling, and delivery concepts as complex workflows. Composition is split
across different action shapes, data commonly moves through mutable workflow
state, and the model does not provide durable command history for long-running
or adaptive work.

A registry census supports optimizing the common case:

- 613 current packages contained 616 workflows.
- The median workflow had one node and one step.
- 502 workflows were singletons; only 23 were branching DAGs.
- 379 packages (61.8%) had one workflow, one node, and one step.
- JSSG appeared in 530 packages (86.5%).
- 52 parent packages accounted for 943 nested-codemod actions.
- Only 12 packages declared or directly used workflow state.

The engine still needs to support the less common cases: fixed multi-step
pipelines, safe parallel reads, dynamic fan-out, agents, approvals, resumability,
and repository-wide coordination. The goal is therefore to simplify authoring,
not to remove orchestration.

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

`exec()`, `jssg()`, and `ai()` all describe `Runnable<Input, Output>` values.
They return plain JSON data checked by schemas, including libraries such as Zod.
This replaces action-specific state with normal typed inputs and outputs. Both
orchestration modes use the same operations and executor.

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
const migrate = jssg({ name: "migrate", package: "@codemod/migrate", input: Project });

export default workflow(async (w) => {
  const project = await w.run(inspect);
  if (project.needsMigration) await w.run(migrate, { input: project });
  return project;
});
```

Plans permit parallelism only through explicit `parallel(...)` groups whose
members are read-only. This is stricter than relying on graph shape alone: the
engine rejects parallel writers because they could change the same files.
Writable operations stay sequential until there is a clear isolation and merge
model.

## Ownership

TypeScript owns the author-facing model and the parts that need rapid iteration:

- runnable definitions and schema-based typing
- workflow and plan authoring
- Plan IR
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

Rust returns a completion with the same command id, a status, and plain output.
The small bridge uses files for this exchange because non-CLI crates must not
write protocol messages to the terminal. It builds without the full Codemod CLI.

## Replay Model

History is an ordered execution record, not only a result cache. Each `w.run()`
has a stable command id and stores the operation details, its completion, and
finally the workflow output. A normal run looks like this:

```text
run inspect -> record request and result
run migrate -> record request and result
return summary -> record final output
```

On a later run, the same `inspect` and `migrate` calls return their recorded
results without executing again. If a command is changed, moved, added, or
removed, replay stops with a clear nondeterminism error instead of silently
running a different workflow against old history. Repeated uses of one runnable
need explicit ids, such as `lint:client` and `lint:server`.

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

The interfaces `OperationExecutor`, `HistoryStore`, `CommandGate`, and
`EventSink` are migration seams. Move responsibility only when the prototype
provides enough evidence for the production implementation:

1. Move canonical command identity, a versioned session envelope, and
   file-backed history into Rust.
2. Move replay matching and finalization into Rust while TypeScript still
   executes workflow operations through the gate.
3. Let the Rust gate execute operations internally and add JSSG, AI, and Plan
   adapters.
4. Run procedural workflow bundles in a restricted QuickJS context before
   treating them as durable or untrusted.

Workflow bodies, runnable typing, schemas, plan authoring, and test ergonomics
should remain TypeScript. This keeps Rust focused on durable engine concerns
without freezing the authoring API too early.

## Evaluation

Before production adoption, validate the design against representative registry
packages: a single JSSG transform, a nested bundle, a command/JSSG hybrid, a
stateful repository analysis, and an agent workflow. The prototype should prove
that simple packages become smaller without hiding the controls needed by the
complex cases.

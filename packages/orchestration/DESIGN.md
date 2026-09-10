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

## Proposal

Use typed operations as the common unit and provide two orchestration modes:

- `plan(...)` creates a fixed, serializable graph at package build time.
- `workflow(async (w) => ...)` runs procedural TypeScript whose calls to
  `w.run(...)` are recorded and replayed.

`exec()`, `jssg()`, and `ai()` all describe `Runnable<Input, Output>` values.
They return plain JSON data validated with Standard Schema, so operations can be
composed without action-specific state APIs. Static and procedural orchestration
share the same executor, repository rules, result types, and test harness.

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
members are declared read-only. Writable operations remain sequential until the
engine has an isolation and merge model.

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

The boundary is intentionally plain JSON: `OperationRequest` enters an executor
and `OperationCompletion` comes back. Completion metadata stays separate from
operation output.

## Replay Model

Every `w.run(...)` creates a command with a stable id and canonical JSON
identity. History is append-only and records scheduling, completion, and final
workflow output. A later run replays matching completions and reports changed,
reordered, added, removed, or output nondeterminism.

This prototype detects nondeterminism after the fact. Workflow functions run
directly in Node and can access time, randomness, the filesystem, the network,
and process state. Durable or untrusted execution requires moving workflow code
into a restricted QuickJS host that exposes only deterministic APIs such as
`w.run(...)`.

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

# @codemod.com/orchestration (prototype)

TypeScript-first prototype of the Codemod orchestration runtime. Workflows are
plain async TypeScript; every operation is issued through `w.run`, recorded in
an append-only history, and replayed from that history on later runs. The only
Rust involved is a tiny execution bridge that runs `exec` operations through
the existing `butterflow_runners::DirectRunner` (see `RUST_BRIDGE.md`).
The evidence, problem statement, proposal, boundaries, and migration path are
summarized in `DESIGN.md`.

## Layout

```
packages/orchestration/
  DESIGN.md         problem statement, proposal, scope, and migration path
  src/protocol.ts   versioned JSON OperationRequest / OperationCompletion
  src/runnable.ts   exec / jssg / ai descriptors (typed via Standard Schema)
  src/target.ts     validation and normalization of a JSSG invocation Target
  src/plan.ts       plan(...) and parallel(...) groups + JSON IR
  src/workflow.ts   workflow(async (w) => ...) and run(executable, options)
  src/history.ts    HistoryStore seam + MemoryHistoryStore
  src/gate.ts       CommandGate seam + ReplayGate (replay matching, errors)
  src/executor.ts   OperationExecutor seam + BridgeExecutor (spawns Rust bridge)
  src/events.ts     EventSink seam
  src/harness.ts    test harness with scripted completions
  fixtures/protocol shared JSON fixtures checked by TS and Rust tests
  tests/            scenario tests (fast, no Rust) + bridge.e2e.test.ts
crates/execution-bridge/  serde structs + Runner call (no orchestration logic)
  src/main.rs             `butterflow-execution-bridge <request.json> <response.json>`
```

## Authoring

```ts
import { exec, jssg, plan, parallel, workflow } from "@codemod.com/orchestration";

const inspect = exec({ name: "inspect", command: "node inspect.js", output: Project });
const migrate = jssg({ name: "migrate", package: "@codemod/migrate", input: Project, output: Summary });

export default workflow(async (w) => {
  const project = await w.run(inspect);
  if (project.needsMigration) await w.run(migrate, { input: project });
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
export default workflow(async (w) => {
  for (const pkg of packages) {
    await w.run(migrate({ target: { root: pkg.path } }), { input: pkg, id: `migrate:${pkg.name}` });
  }
});
```

- `exec` output: with an `output` schema, the runner's returned text is parsed
  as JSON and validated; without one the output is `{ stdout }`. The field name
  is provisional: the existing `DirectRunner` combines stdout and stderr on
  Unix but returns stdout alone on other platforms.
- Repeated calls of the same runnable need an explicit id, for example
  `w.run(lint, { id: "lint:" + i })`. A repeated call without an id throws
  `DuplicateCommandIdError`. Unique calls use the runnable name as their id.
- Non-success completions (`failed`, `cancelled`, `unknown`) throw
  `OperationError` from `w.run`; catch it to branch.
- Workflow return values and operation outputs are plain JSON.
- `parallel(...)` is an author assertion. The prototype starts each member as a
  whole concurrent operation; it does not implement per-file locking. Do not
  place dependent mutations or opaque commands that may conflict in one group.
  `DESIGN.md` describes the future JSSG file scheduler.
- A JSSG definition is callable with exactly `{ target: { root?, include?, exclude? } }`
  and returns a targeted runnable; `input` and `id` still go to `w.run`. The
  target is validated when bound (relative `root` without `..`, non-empty
  pattern lists, no unknown fields, not empty), recorded in history as command
  content, and sent on the wire as `operation.target`. Changing it under the
  same id replays as `changed`. `exec` and `ai` are not callable, a targeted
  runnable cannot be targeted again, and `w.run` rejects a `target` option, so
  a target is never silently ignored. There is no generic `target()` wrapper
  and no `shard()`/`scope()` helper; sharding and worker counts are scheduler
  behavior.
- No adapter enforces a target yet: the bridge decodes it and answers "no
  adapter" for `jssg`, and every `exec` still runs in the executor's `cwd`.
  See `DESIGN.md` ("Targeting") for the proposed callable form.

## Running

```ts
import { BridgeExecutor, MemoryHistoryStore, run } from "@codemod.com/orchestration";

const executor = new BridgeExecutor({ bin: "target/debug/butterflow-execution-bridge", cwd: repoDir });
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

## Determinism warning

Normal Node execution is NOT a secure deterministic sandbox. Workflow code only
receives `w.run`, but nothing prevents it from reading `Date`, `Math.random`,
`fs`, or `process`. Determinism is validated after the fact by replay
comparison; if a workflow uses such inputs the replay will fail with
`NondeterminismError`. QuickJS sandboxing is out of scope for this prototype.

## Migration seams

`OperationExecutor`, `HistoryStore`, `CommandGate`, and `EventSink` are small
interfaces with JSON-only inputs and outputs. Each in-memory implementation can
move to Rust one at a time without changing workflow source. `jssg` and `ai`
descriptors exist but have no executor adapter yet; the bridge decodes them
(including a JSSG `target`), returns a `failed` completion, and the harness
scripts their results.

## Commands

```
pnpm install
pnpm --filter @codemod.com/orchestration test          # fast TS tests, no Rust
pnpm --filter @codemod.com/orchestration typecheck
pnpm --filter @codemod.com/orchestration test:e2e     # builds only the bridge crate, then cross-language test
cargo test -p butterflow-execution-bridge              # Rust protocol, runner, and binary tests
```

The full `codemod` CLI is never built or used by this package.

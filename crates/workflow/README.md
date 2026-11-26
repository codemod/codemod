# codemod-workflow

The workflow engine. It runs `workflow.ts` files, and `workflow.yaml` files
through a translation to the same TypeScript. User docs:
`docs/community/workflows/reference.mdx`.

```ts
import { step, forEachFile } from "codemod/workflow";
import { parse } from "codemod/ast-grep";
import migrate from "./scripts/migrate.ts";

export default async function main() {
  const usages = await step("Find moment imports",
    forEachFile({ language: "typescript", containing: "moment" }, (file) => {
      const found = parse(file).root().findAll({ rule: { pattern: 'import $X from "moment"' } });
      return found.length ? file.path : undefined;
    }),
  );
  await step(forEachFile({ include: usages.outputs }, migrate));
}
```

## Layout

| Path | What it does |
|---|---|
| `src/build.rs` | oxc build: extracts `forEachFile` and `dependencyGraph` callbacks and function steps, turns outside variables into captures, reports the callback rules and body import rules as build errors; `check_imports` walks the import graphs of packages the body uses; `inspect` reads a module's task calls without running it |
| `src/run.rs`, `src/js.rs`, `src/js/workflow.ts`, `src/js/body_globals.ts` | The workflow body in one QuickJS context with pure modules and replay-safe `Date`/`Math.random`; the `codemod/workflow` runtime: `step()`, replay by position and content, state, events, cancellation |
| `src/tasks/isolate.rs`, `src/tasks/for_each_file.rs`, `src/tasks/function.rs` | Callbacks and function steps on sandbox workers (`codemod_sandbox::Worker`): the JSSG module set, async, imports, metrics and state |
| `src/engine.rs` | `Engine::start` / `resume` / `cancel` over a `RunStore` (`crates/state`) |
| `src/tasks/` | Host tasks, each a thin adapter over an existing executor: `forEachFile`, `astGrep`, `shell`, `shard`, `codemod`, `agent`, `installSkill`, `approval`, `branch` / `commit` / `pullRequest` (imported from `codemod/git`, `src/js/git.ts`), and `dependencyGraph` (over the `codemod-deps` crate) |
| `src/js/deps.ts`, `src/deps_host.rs`, `src/js/dependencies.ts`, `src/js/semver.ts` | `codemod/deps`: `dependencyGraph`, `registry` and `semver` in the body (`deps.ts`) and in isolates (`dependencies.ts`), batching lookups started together; the host side over `Services::package_registry` |
| `src/assess/`, `src/js/assess.ts` | `codemod/assess`: typed questions about a file in callbacks and function steps (`questions.rs`), the context assembled from its imports through re-exports and its dependents (`context.rs`), System One and OpenAI Decisions over HTTP with retries (`http.rs`), and which model answers (`config.rs`); the run's model is `Services::decision` |
| `src/tasks/changes.rs` | One commit path for every task that edits files: conflict checks, dry run, diffs |
| `src/target.rs`, `src/tasks/overlay.rs` | Runs on a read-only snapshot a host keeps (`Engine::start_on_snapshot`, `TargetFiles`): steps select and read through the dry-run overlay, never the disk; tasks that only read the disk (`dependencyGraph`, `shard`, workspace semantic analysis) refuse; the host's per-file outcome cache (`FileCache`) and output sink (`FileOutputs`) |
| `src/compat/` | workflow.yaml to TypeScript; `${{ }}` expressions become TypeScript or fall back to `evaluate()`, which uses the YAML engine's evaluator |
| `src/events.rs`, `src/interaction.rs` | What the engine reports, and the questions it asks; the CLI implements both |

The engine never writes to the terminal: the CLI (`crates/cli/src/workflow_exec.rs`)
renders events and answers prompts.

## Not done yet

- On a snapshot: `dependencyGraph`, `shard` and workspace semantic analysis,
  and the dependents `assess()` adds to a file's context.
- The per-file cache for jssg transforms run through the workflow.yaml
  translation's adapter (`jssgFile`).

- A TUI on top of step events.
- Step results captured by reference: captures are copied into each step's
  history entry.
- `codemod/llm` inside JSSG transforms, callbacks and function steps.
- Creating or changing other files from a `forEachFile` callback (`jssgTransform`, `fs` writes).
  Jssg transforms from `workflow.yaml` files may still change other files with
  `jssgTransform`, through the translation's adapter, as before.
- `dependencyGraph` and
  `registry` on the Node.js host.
- One rate limit for registry requests across the whole run: each batch of
  lookups is capped, but batches from parallel workers are not.
- Crash-safe file commits, and conflict checks between steps running in parallel.

## Run the tests

```bash
cargo test -p codemod-workflow
```

# codemod-workflow

The workflow engine. It runs `workflow.ts` files, and `workflow.yaml` files
through a translation to the same TypeScript. User docs:
`docs/community/workflows/reference.mdx`.

```ts
import { step, forEachFile, parse, jssg } from "codemod:workflow";

export default async function main() {
  const usages = await step("Find moment imports",
    forEachFile({ language: "typescript", containing: "moment" }, (file) => {
      const found = parse(file).root().findAll({ rule: { pattern: 'import $X from "moment"' } });
      return found.length ? file.path : undefined;
    }),
  );
  await step(jssg("./scripts/codemod.ts", { files: usages.outputs }));
}
```

## Layout

| Path | What it does |
|---|---|
| `src/build.rs` | oxc build: extracts `forEachFile` callbacks, turns outside variables into captures, reports the callback rules as build errors; `inspect` reads a module's task calls without running it |
| `src/run.rs`, `src/js/workflow.js` | The `codemod:workflow` runtime in one QuickJS context: `step()`, replay by position and content, state, events, cancellation |
| `src/engine.rs` | `Engine::start` / `resume` / `cancel` over a `RunStore` (`crates/state`) |
| `src/tasks/` | Host tasks, each a thin adapter over an existing executor: `forEachFile`, `jssg` (`butterflow_core::jssg_runner`), `astGrep`, `shell`, `shard`, `codemod`, `agent`, `installSkill`, `approval`, `branch` / `commit` / `pullRequest` |
| `src/tasks/changes.rs` | One commit path for every task that edits files: conflict checks, dry run, diffs |
| `src/compat/` | workflow.yaml to TypeScript; `${{ }}` expressions become TypeScript or fall back to `evaluate()`, which uses the YAML engine's evaluator |
| `src/events.rs`, `src/interaction.rs` | What the engine reports, and the questions it asks; the CLI implements both |

The engine never writes to the terminal: the CLI (`crates/cli/src/workflow_exec.rs`)
renders events and answers prompts.

## Not done yet

- A TUI on top of step events.
- Step results captured by reference: captures are copied into each step's
  history entry.
- `codemod:llm` inside JSSG transforms run by the `jssg` task.
- Async `forEachFile` callbacks, and creating or reading other files from one.
- Crash-safe file commits, and conflict checks between steps running in parallel.

## Run the tests

```bash
cargo test -p codemod-workflow
```

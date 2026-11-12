# codemod-flow (spike)

A throwaway spike of the `step()` / `forEachFile` workflow API. It is not wired
into the CLI and not published. Design doc: "Design: step() and forEachFile for
TypeScript workflows" in Notion.

```ts
import { step, forEachFile, parse } from "codemod:flow";

const LEGACY = "moment";

export default async function main() {
  const usages = await step("Find moment imports",
    forEachFile({ language: "typescript", containing: LEGACY }, (file) => {
      const found = parse(file).root().findAll({ rule: { pattern: `import $X from "${LEGACY}"` } });
      return found.length ? file.path : undefined;
    }),
  );

  await step(forEachFile({ language: "typescript" }, (file) => {
    if (!usages.outputs.includes(file.path)) return;   // captured step result
    file.write(rewrite(parse(file)));
  }));
}
```

## How it works

**Build** (`src/build.rs`) uses oxc's semantic analysis:

- finds every `forEachFile(selector, callback)` whose `forEachFile` is imported
  from `codemod:flow`;
- extracts the callback into its own module, copying in the module-level
  declarations it uses (transitively) and the `codemod:*` imports it needs;
- turns locals of enclosing functions into captures, and rewrites the call site
  to `__flowCallback(id, () => ({ ...captures }))`;
- rejects, with a file:line:column error: assigning to an outside variable,
  capturing a reassigned local, capturing a function or class (including a
  `const` holding one), using a reassigned module-level binding, starting a
  step inside a callback, and passing a callback by name.

**Run** (`src/run.rs`, `src/js/flow.js`):

- the workflow body runs in one QuickJS context; `step()` starts its task
  immediately, so `Promise.all` runs steps at the same time;
- `forEachFile` tasks run on host threads; a small loop settles their promises;
- captures are checked as strict JSON when the step starts, and every step
  result is deep-frozen;
- history is keyed by position plus a canonical form of the task. A recorded
  success is replayed if its content matches and is a `NondeterminismError` if
  not; a recorded failure is run again. With `history_path` set, history is
  rewritten after every step.

**Files** (`src/files.rs`, `src/js/callback_prelude.js`):

- selection: globs (or the language's extensions), `.gitignore`, then the
  `containing` text filter; contents are read once up front (snapshot);
- the callback runs on a pool of threads, one QuickJS context per thread, with
  the module and captures loaded once per context;
- `file.write`, `file.rename` and `file.delete` record intents; the step
  checks for conflicts and then applies all changes, or none on a conflict;
- a callback that throws fails only its file, reported in `failures`.

## Left out

- `shell` and `agent` tasks, the `matching` selector, async callbacks, and
  creating or reading other files from a callback.
- Step results captured by reference: captures are inline, so a later step's
  history entry repeats the earlier result.
- CLI wiring, progress output, timeouts and memory limits, `console` in
  callbacks, imports of local modules.
- Crash-safe commits, and conflict checks between steps running in parallel.
- Windows: nothing here is Unix-specific, but it was not tested.

## Run

```bash
cargo test -p codemod-flow
```

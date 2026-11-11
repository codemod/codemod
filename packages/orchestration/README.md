# @codemod.com/orchestration (prototype)

TypeScript workflows for the `codemod` CLI. A workflow is a plain async
TypeScript module. Calling a step creates a command; awaiting it inside the
workflow runs it. Every command is recorded in a history, so a run can be
resumed and replayed. `codemod workflow run -w workflow.ts` bundles the
workflow and runs it in one QuickJS context inside the `codemod` binary, with
no Node at run time.

There are four kinds of step:

- `shell`: a command in the target directory.
- `jssg`: a JSSG transform written inline next to the workflow.
- `agent`: an agent task, on the built-in Butterflow agent or the installed
  Claude Code or Codex CLI.
- `assessment`: typed questions about each matched file, answered by a
  decision model.

## Writing a workflow

```ts
import { dynamic, jssg, parallel, sequence, shell } from "@codemod.com/orchestration";
import { rewriteSignal } from "./helpers.ts"; // bundled into the transform

const inspect = shell({ name: "inspect", command: "node inspect.js", output: Project });
const migrate = jssg({
  name: "migrate-signals",
  language: "tsx",
  include: ["src/**/*.tsx"],
  semanticAnalysis: "workspace",
  selector: { rule: { pattern: "createSignal($VALUE)" } }, // optional static prefilter
  input: Project,
  output: Summaries,
  transform(root, options) {
    // root: SgRoot<TSX>, options.params.input: Project
    const edits = root.root().findAll({ rule: { pattern: "createSignal($VALUE)" } }).map(rewriteSignal);
    return { content: root.root().commitEdits(edits), output: { file: root.relativeFilename() } };
  },
});

export default dynamic(async () => {
  const project = await inspect();
  if (project.needsMigration) await migrate({ input: project });
  return project;
});

// Static plans: no-input steps ignore the preceding output.
export default sequence(rename(), updateImports(), format());
export default sequence(
  parallel(countTodos(), countFixmes()),
  dynamic((counts) => counts.reduce((total, count) => total + count, 0)),
);

// A JSSG invocation can narrow its files with a target.
const web = { root: "apps/web", include: ["src/**"], exclude: ["**/generated/**"] };
export default parallel(transformA({ target: web }), transformB({ target: web }));
```

- Calling a step creates a `Command`: `inspect()`, `lint({ id })`,
  `migrate({ input, target, id })`. Creating it does nothing. Awaiting it
  inside a workflow runs it once and returns its typed output; awaiting it
  outside a run rejects with `NoActiveRunError`.
- A module may export a single step as its default, for example
  `export default shell({...})`.
- `shell`, `agent` and `assessment` invocations accept `id` and `input`.
  `jssg` invocations also accept `target` (`{ root?, include?, exclude? }`,
  relative to `--target`). Any other field throws when the command is created.
- Repeated invocations of one step need distinct ids, for example
  `lint({ id: "lint:" + i })`; otherwise `DuplicateCommandIdError`.
- A failed, cancelled or unknown step rejects with `OperationError`; catch it
  to branch. `error.details` says what failed (for JSSG, the phase).
- Inputs, outputs and the workflow's return value are plain JSON. With an
  `output` schema, `shell` output is parsed as JSON and validated; without one
  it is `{ stdout }`.
- `sequence(...)` returns its last output. `parallel(...)` passes the same
  input to every member and returns a tuple in declaration order. It says which
  steps may overlap, not how many run at once: the runtime admits a bounded
  number (override with `CODEMOD_ORCHESTRATION_CAPACITY`). Don't put steps that
  edit the same files in one `parallel` group.
- `dynamic(...)` marks arbitrary TypeScript; raw functions are rejected as
  stages.

### The inline transform

`transform` has the same `(root, options)` contract as a standalone codemod's
default export (https://docs.codemod.com/jssg/reference), typed by `language`.
It may return `string | null | undefined`, or `{ content?, output }`, where the
`output` values come back from the step as an array in file order. Invocation
input reaches the transform as `options.params.input`. Without `include`, a
step applies to the language's file extensions, as a YAML `js-ast-grep` step
does. `semanticAnalysis` is `"file"`, `"workspace"`, or
`{ mode, root? }`.

The workflow and its transforms are written together but never run together.
At build time the CLI splits every `jssg({ ... })` call out of the module and
bundles each transform, with the helpers it imports, into its own artifact
named by content hash (`crates/orchestration/src/build`). So a transform may
use its own locals, globals, and bindings imported from other modules. It may
not use anything else declared in the workflow file, or imports from
`@codemod.com/orchestration`; the build fails with a source position. Move
shared helpers into a module and import them.

Each JSSG step selects its files, transforms every file from the same snapshot,
checks the edits for conflicts, and only then writes. A failure before the
write leaves every file unchanged. `write()` and `jssgTransform()` are staged
with the step's other edits.

### The static selector

`selector` is optional ast-grep rule data, `{ rule, constraints?, utils? }`. It
is evaluated natively on each selected file before the file reaches JavaScript;
files without a match are skipped. It only filters files: the transform finds
its nodes with `root.find` / `root.findAll`, and `options.matches` is not set.

## Agent and assessment steps

```ts
import { agent, assessment, dynamic } from "@codemod.com/orchestration";

const assessSources = assessment({
  name: "assess-sources",
  include: ["src/**/*.ts"],
  exclude: ["**/*.d.ts"],
  ask: ({ file }) => ({
    route: {
      type: "choice" as const,
      instructions: `Which migration route best fits ${file.path}?`,
      criteria: {
        codemod: "A mechanical AST transformation is sufficient",
        agent: "Repository context or coordinated edits are needed",
        manual: "The evidence is insufficient for safe automation",
      },
    },
  }),
});

const finish = agent({
  name: "finish-migration",
  prompt: "Finish the migration and make the tests pass.",
  input: Change,
  backend: { kind: "codex", sandbox: "workspace-write" }, // omit for the built-in agent
});

export default dynamic(async () => {
  const results = await assessSources(); // [{ file, assessment }] in file order
  const manual = results.some(({ assessment: { answers: { route } } }) =>
    route.type === "refused" || route.choice === "manual" || route.confidence < 0.75,
  );
  if (!manual) await finish({ input: change });
  return { files: results.length, manual };
});
```

- `assessment` sends only the matched files' contents to the model, one request
  per file, and changes nothing. Questions are `choice`, `score` or `yesno`;
  any answer may be `{ type: "refused" }`. Routing on the answers is workflow
  code.
- The decision model comes from the environment: `DECISION_PROVIDER`
  (`systemone`, the default, or `openai`), `DECISION_API_KEY`, and optionally
  `DECISION_MODEL`, `DECISION_BASE_URL`, `DECISION_PATH`,
  `DECISION_AUTH_HEADER`. Workflow code never names an endpoint or a key.
- `agent` returns `{ text }`, or JSON validated against its `output` schema.
  Backends: `builtin` (codemod-ai; `LLM_API_KEY`, `LLM_PROVIDER`, `LLM_MODEL`,
  `LLM_BASE_URL`; settings `tools`, `maxSteps`), `claude-code` (the logged-in
  `claude` CLI; setting `tools`) and `codex` (the logged-in `codex` CLI;
  setting `sandbox`). External CLIs run non-interactively, without bypass
  flags or user and project settings.
- Each agent step runs in a `codemod-agent` process in `--target`, found
  through `CODEMOD_AGENT_BIN` or next to the `codemod` binary
  (`pnpm build:agent`). Cancelling the run kills it and its process tree.

## Running

```sh
cargo build -p codemod                    # the runtime is built into the CLI
cargo build -p orchestration-agent        # only for agent() steps
target/debug/codemod workflow run -w ./path/to/workflow.ts --target ./repository
```

```text
codemod workflow run -w <workflow.ts|.mts> [--target <directory>] [--input <json>] [--history <file>]
```

- `--target` (default: the current directory) is where `shell` and `agent` run
  and what JSSG targets are relative to.
- `--input <json>` is the root input when the workflow declares one.
- `--history <file>` resumes from the file when it exists and writes the run's
  history back to it.
- The final value prints as JSON on stdout. Errors, and workflow and transform
  console output, go to stderr; a failed run exits 1.
- SIGINT, SIGTERM and SIGHUP abort the run: child process trees are killed and
  the step in flight is recorded as `cancelled` with nothing written (a running
  `agent` is `unknown`, since it may have edited files).
- `@codemod.com/orchestration` imports resolve to the runtime built into the
  binary; `CODEMOD_ORCHESTRATION_SRC=<path to src>` uses other sources, for
  developing the runtime. A `node:*` import fails the build.
- Bundles are cached under `$CODEMOD_WORKFLOW_CACHE` (default
  `<tmpdir>/codemod-workflow-cache-<uid>`) and rebuilt when the workflow, a
  module it imports, or a bundled helper changes.
- This is a runner for trusted local workflows, not for untrusted registry
  packages. `Date` and `Math.random` are available, so determinism is checked
  by replay, not enforced.

### Replay

A history is the list of commands a run issued, their completions, and the
final output. A later run with the same history replays recorded commands
instead of running them, and runs new ones after them, which is how an
interrupted run resumes. A command is matched by its id and its content (for
JSSG, including the transform's hash), so editing a step, its input or its
transform under the same id fails with `NondeterminismError` (`changed`, and
likewise `reordered`, `removed`, `added`, `output`).

## Layout

Folders under `src/` import one way (`tests/layers.test.ts` enforces this):

```
core       (nothing)            contracts: Host seam, protocol, history, errors
authoring  core                 what workflow code calls: shell, jssg, agent, assessment, sequence, parallel, dynamic
execution  core                 how each step kind runs inside the host
runtime    core, authoring, execution   run(): issue, replay or execute, record history
host       core, authoring, execution, runtime   the bundle entry and Host inside QuickJS
```

Rust side: `crates/orchestration` (build split and bundling, the QuickJS host,
the native file walker, JSSG batch rules), `crates/orchestration-agent`
(`codemod-agent`), and `crates/cli/src/commands/workflow/typescript.rs`.

## Commands

```
pnpm --filter @codemod.com/orchestration test          # TS specs (fake agent runner)
pnpm --filter @codemod.com/orchestration typecheck
cargo test --release -p codemod --test typescript_workflow --test jssg_parity   # end to end
cargo test -p orchestration                            # build split, bundles, host, walker, batch rules
cargo test -p orchestration-agent                      # agent runner and external CLIs
```

Live tests call real services and are opt-in, one flag per backend; without
the flag they are skipped.

```
CODEMOD_LIVE_ASSESSMENT=systemone,openai DECISION_API_KEY=... pnpm --filter @codemod.com/orchestration test:live:assessment
LLM_API_KEY=... pnpm --filter @codemod.com/orchestration test:live:agent:builtin
pnpm --filter @codemod.com/orchestration test:live:agent:claude-code
pnpm --filter @codemod.com/orchestration test:live:agent:codex
```

Behind a proxy, add `NODE_USE_ENV_PROXY=1` to the assessment command.

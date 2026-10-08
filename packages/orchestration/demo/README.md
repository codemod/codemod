# Orchestration demo

Ten small workflows over one story: a project calls a deprecated `oldApi(name)`
and must move to `newApi({ name })`. Each file is a complete workflow that
`codemod-workflow` runs against a copy of `fixture/`.

| file          | shows                                                                                         |
| ------------- | --------------------------------------------------------------------------------------------- |
| `01-shell.ts`    | one `shell` step as the whole workflow                                                        |
| `02-jssg.ts`     | one read-only `jssg` step: `include`/`exclude`, per-file outputs aggregated into an array     |
| `03-sequence.ts` | two ordered migrations (one imported from `lib/steps.ts`) and a verification, typed data flow |
| `04-parallel.ts` | three independent read-only analyses, tuple output in declaration order                       |
| `05-composed.ts` | shell, parallel analyses, migration, parallel verification, as one static plan                |
| `06-dynamic.ts`  | a `dynamic` step that branches on shell output and awaits a static group                      |
| `07-input.ts`    | a root that requires input, supplied with `--input <json>`                                    |
| `08-assessment.ts` | file-oriented read-only TypeSafe assessment (one question set per file)                   |
| `09-agent.ts`      | a restricted Claude Code agent followed by deterministic verification                    |
| `10-assisted.ts`   | assessment-driven routing to codemod, agent, or manual review                             |

Supporting modules: `lib/schemas.ts` (Standard Schema guards and the types they
carry), `lib/ast.ts` (helpers the transforms call; bundled into the artifacts),
`lib/steps.ts` (steps shared by several workflows). `fixture/` is the target
project: `src/app.ts` and `src/utils.ts` call `oldApi`, `src/config.ts` calls
nothing, `src/legacy.generated.ts` and `src/globals.d.ts` are excluded by every
step.

## Setup

Node 24 and a Rust toolchain. Workflows 01-07 need no network access or installs
beyond the monorepo's existing `node_modules`. Workflow 08 requires
`TYPESAFE_API_KEY`, workflow 09 requires an authenticated local Claude Code
CLI, and workflow 10 requires both.

```sh
# from the repository root: build the Rust execution bridge (once)
cargo build -p butterflow-execution-bridge

cd packages/orchestration
```

The target is a copy of the fixture outside the repository. It cannot live
under `packages/orchestration/demo/`: the file walker honors ignore files in
ancestor directories, and the repository's `.gitignore` ignores `target/`.
Every command below starts from a fresh copy; run this before each workflow
(or between runs, to compare with an already migrated target):

```sh
rm -rf /tmp/codemod-demo && cp -R demo/fixture /tmp/codemod-demo
```

`node bin/codemod-workflow.mjs` is the CLI. `pnpm --filter @codemod.com/orchestration workflow`
is the same thing. It prints the workflow's final value as JSON; errors go to
stderr with exit code 1. To see what a run changed:

```sh
diff -ru demo/fixture /tmp/codemod-demo
```

## `01-shell.ts`

```sh
rm -rf /tmp/codemod-demo && cp -R demo/fixture /tmp/codemod-demo
node bin/codemod-workflow.mjs demo/01-shell.ts --target /tmp/codemod-demo
```

```json
{
  "stdout": "src/app.ts\nsrc/legacy.generated.ts\nsrc/utils.ts\n"
}
```

A shell step without an `output` schema yields `{ stdout }`. The command runs
in the target directory.

## `02-jssg.ts`

```sh
rm -rf /tmp/codemod-demo && cp -R demo/fixture /tmp/codemod-demo
node bin/codemod-workflow.mjs demo/02-jssg.ts --target /tmp/codemod-demo
```

```json
[
  { "file": "src/app.ts", "oldApi": 2, "newApi": 0 },
  { "file": "src/config.ts", "oldApi": 0, "newApi": 0 },
  { "file": "src/utils.ts", "oldApi": 1, "newApi": 1 }
]
```

One entry per selected file, in file order. `legacy.generated.ts` and
`globals.d.ts` are excluded by the definition. Nothing is written.

## `03-sequence.ts`

```sh
rm -rf /tmp/codemod-demo && cp -R demo/fixture /tmp/codemod-demo
node bin/codemod-workflow.mjs demo/03-sequence.ts --target /tmp/codemod-demo
```

```json
{
  "migrated": 2,
  "remaining": 0
}
```

`rename-calls` (shared, from `lib/steps.ts`) rewrites `oldApi(x)` to
`newApi(x)` and outputs `Migration[]`; `wrap-options` takes that array as its
input and rewrites `newApi(x)` to `newApi({ name: x })` only in those files;
`verify-migrations` builds its command from the second array and checks the
sources. `diff -ru demo/fixture /tmp/codemod-demo` shows the three rewritten
call sites in `app.ts` and `utils.ts`; `legacy.generated.ts` is untouched.

The migration is idempotent. Running it again without a reset prints
`{ "migrated": 0, "remaining": 0 }`.

## `04-parallel.ts`

```sh
rm -rf /tmp/codemod-demo && cp -R demo/fixture /tmp/codemod-demo
node bin/codemod-workflow.mjs demo/04-parallel.ts --target /tmp/codemod-demo
```

```json
[
  [
    { "file": "src/app.ts", "calls": 2 },
    { "file": "src/utils.ts", "calls": 1 }
  ],
  [{ "file": "src/utils.ts", "calls": 1 }],
  []
]
```

Files calling `oldApi`, files calling `newApi`, files calling `newApi` with a
bare argument: three read-only analyses, one tuple in declaration order.

## `05-composed.ts`

```sh
rm -rf /tmp/codemod-demo && cp -R demo/fixture /tmp/codemod-demo
node bin/codemod-workflow.mjs demo/05-composed.ts --target /tmp/codemod-demo
```

```json
[
  { "remaining": 0 },
  { "migrated": 2, "remaining": 0 }
]
```

`inspect`, then the three analyses in parallel, then `rename-calls`, then two
verifications in parallel. A sequence returns its last stage's output: here
the tuple of the final group. The whole plan is static data; `sequence(...)`
and `parallel(...)` build it before anything runs.

## `06-dynamic.ts`

```sh
rm -rf /tmp/codemod-demo && cp -R demo/fixture /tmp/codemod-demo
node bin/codemod-workflow.mjs demo/06-dynamic.ts --target /tmp/codemod-demo
```

```json
{
  "pending": 2,
  "migrated": [
    { "file": "src/app.ts", "replaced": 2 },
    { "file": "src/utils.ts", "replaced": 1 }
  ],
  "remaining": 0,
  "usage": [
    { "file": "src/app.ts", "calls": 2 },
    { "file": "src/utils.ts", "calls": 2 }
  ]
}
```

Run it again without a reset: `inspect` finds nothing pending, the migration is
skipped, and the verification group still runs.

```json
{
  "pending": 0,
  "migrated": [],
  "remaining": 0,
  "usage": [
    { "file": "src/app.ts", "calls": 2 },
    { "file": "src/utils.ts", "calls": 2 }
  ]
}
```

## `07-input.ts`: root input

The first stage declares an `input` schema, so the workflow needs a value.

```sh
rm -rf /tmp/codemod-demo && cp -R demo/fixture /tmp/codemod-demo
node bin/codemod-workflow.mjs demo/07-input.ts --target /tmp/codemod-demo --input '{"replacement":"newApi"}'
```

```json
{
  "migrated": 2,
  "remaining": 0
}
```

Without `--input` the CLI refuses to start (exit code 1):

```text
workflow requires an input value; pass --input <json>
```

`--input null` is a value, not an absent input; the schema rejects it before
anything runs, as it does any other wrong shape:

```text
input of 'migrate-to': expected Config
```

`--input` is strict JSON:

```sh
node bin/codemod-workflow.mjs demo/07-input.ts --target /tmp/codemod-demo --input '{replacement: newApi}'
```

```text
--input must be valid JSON (Expected property name or '}' in JSON at position 1 (line 1 column 2)); got: {replacement: newApi}
usage: codemod-workflow <workflow.ts> [--target <directory>] [--bridge <binary>] [--input <json>]
```

## `08-assessment.ts`: file-oriented assessment

This workflow defines a file-oriented assessment that selects TypeScript
sources, reads each one, and asks a System One model three typed questions per
file. The model receives each file's path and content automatically — the
`ask` function returns QUESTIONS ONLY. The include/exclude globs are the
privacy boundary: only matched files are sent to the model.

```sh
rm -rf /tmp/codemod-demo && cp -R demo/fixture /tmp/codemod-demo
TYPESAFE_API_KEY=... node bin/codemod-workflow.mjs demo/08-assessment.ts --target /tmp/codemod-demo
```

Assessment is a first-class Runnable — it can be the root directly (no
`dynamic()` wrapper needed), and composes naturally in `sequence()` and
`parallel()`. The result is an ordered `Array<{ file, assessment }>`, one entry
per matched file. Each assessment contains the model version, usage, and typed
answers for `route`, `risk`, and `safeToAutomate`, including probabilities and
confidence. It is evidence for later workflow code, not a command to mutate
files.

## `09-agent.ts`: agent work, deterministic verification

The installed Claude Code CLI receives only read and file-editing tools. It
migrates the target, then `verifyNoLegacy` checks the repository without trusting
the agent's final message.

```sh
claude auth status
rm -rf /tmp/codemod-demo && cp -R demo/fixture /tmp/codemod-demo
node bin/codemod-workflow.mjs demo/09-agent.ts --target /tmp/codemod-demo
```

```json
{ "remaining": 0 }
```

The backend can be changed to `builtin` or `codex` in the workflow. Each
backend has its own enforceable settings; the example uses Claude Code so it
can explicitly omit the shell tool.

## `10-assisted.ts`: assessment-driven routing

This dynamic workflow combines both primitives. `assessment()` selects and
evaluates each source file individually, returning typed per-file
probabilities. Ordinary TypeScript applies the routing policy: if any file is
low-confidence or manual, the whole batch stops for review; otherwise the
workflow chooses the deterministic two-stage codemod or the agent. Both
automatic paths finish with deterministic verification.

```sh
rm -rf /tmp/codemod-demo && cp -R demo/fixture /tmp/codemod-demo
TYPESAFE_API_KEY=... node bin/codemod-workflow.mjs demo/10-assisted.ts --target /tmp/codemod-demo
```

The exact route is intentionally not hard-coded: the assessment result and its
confidence are part of the output. A `manual-review` result leaves the fixture
unchanged.

## How the transforms run

`codemod-workflow` loads a workflow through a build step that finds every
`jssg({ ... })` call in the module graph, bundles its `transform` (with the
helpers it imports, here `lib/ast.ts`) into a standalone artifact, and
replaces the function with the artifact's `{ name, hash }`. The workflow then
runs in Node and each artifact in the bridge's QuickJS sandbox, one bridge
process per command. That is why a transform may use its parameters, globals,
and imported bindings, but not a `const` or `function` declared in the
workflow file: nothing is serialized across the boundary. Dynamic values reach
a transform through its step's `input` (see `wrap-options` and `migrate-to`).

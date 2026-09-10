# Rust execution bridge, explained in TypeScript

All orchestration logic lives in TypeScript. Rust executes `exec` through the
existing `DirectRunner` and local JSSG through the existing QuickJS sandbox.
Planning, replay, and history remain TypeScript responsibilities. The whole
bridge is equivalent to:

```ts
interface ExistingEngineBridge {
  execute(request: OperationRequest): Promise<OperationCompletion>;
}
```

## Files

- `crates/execution-bridge/src/lib.rs`: serde structs mirroring
  `packages/orchestration/src/protocol.ts`, plus `execute` and
  `completion_from_result`.
- `crates/execution-bridge/tests/protocol.rs`: round-trips the shared fixtures in
  `packages/orchestration/fixtures/protocol` and runs `echo`-style commands
  through `DirectRunner`.
- `crates/execution-bridge/tests/jssg.rs`: JSSG adapter behavior (default
  language globs, walker settings, failure status, deferred rename deletion,
  script root resolution) through `execute_in` against temporary repositories.
- `crates/execution-bridge/src/main.rs`: the `butterflow-execution-bridge`
  binary. One-shot file protocol: `<request.json> <response.json>`. It never
  writes to stdout or stderr (non-CLI crates must not), so the "only the CLI
  prints" rule holds. Exit codes: 0 completion written, 2 bad arguments,
  3 unreadable or malformed request, 4 could not write the response.
- `crates/execution-bridge/tests/bin.rs`: runs the binary against temp files.

## Type by type

`Operation` (Rust enum, `#[serde(tag = "kind")]`)

```ts
type Operation =
  | { kind: "exec"; command: string; env?: Record<string, string> }
  | {
      kind: "jssg";
      script: string; // safe relative path; resolved against context.scriptRoot
      language: string;
      include?: string[]; // default: the language's file extensions
      exclude?: string[];
      semanticAnalysis?: "file" | "workspace" | { mode: "file" | "workspace"; root?: string };
      target?: Target;
      input?: Json; // exposed to the transform as options.params.input
    }
  | { kind: "ai"; prompt: string; input?: Json }; // decoded only, never executed here

// Only jssg carries file selection. Definition and invocation filters intersect.
interface Target { root?: string; include?: string[]; exclude?: string[] }
```

`script`, `target.root`, and `semanticAnalysis.root` are validated with one
rule on both sides (`isSafeRelativePath` in `paths.ts`,
`validate_relative_path` in `lib.rs`): non-empty, not absolute on any platform
(`/x`, `\x`, `C:\x`), and no `..` segment; `foo..bar` is an ordinary name.
`semanticAnalysis.root` requires `workspace` mode and is omitted from the
serialized object form when absent.

`OperationRequest` / `RequestContext` / `OperationCompletion` /
`CompletionError` / `CompletionStatus`

```ts
interface OperationRequest {
  protocolVersion: 2; commandId: string; operation: Operation;
  context?: RequestContext; // executor input, never recorded in history
}
interface RequestContext { scriptRoot?: string } // strict: no other fields
type CompletionStatus = "succeeded" | "failed" | "cancelled" | "unknown";
interface CompletionError { message: string; exitCode?: number; output?: string }
interface OperationCompletion {
  protocolVersion: 2; commandId: string; status: CompletionStatus;
  output?: Json; error?: CompletionError;
}
```

Rust field names are snake_case with `#[serde(rename_all = "camelCase")]`, so
the wire JSON is byte-for-byte the TypeScript shape. Optional fields are
omitted when absent. `Operation` and `Target` use `deny_unknown_fields`, and
`isOperation` / `isTarget` in `protocol.ts` apply the same per-variant field
sets, so a `target` on `exec` or `ai` (or any field from another variant) is a
validation error on both sides rather than a silently ignored field.

`parse_request(text)`

```ts
function parseRequest(text: string): OperationRequest {
  const request = JSON.parse(text);
  if (request.protocolVersion !== 2) throw new Error("unsupported protocol version");
  return request;
}
```

`execute(runner, request)`

```ts
async function execute(runner: Runner, request: OperationRequest): Promise<OperationCompletion> {
  if (request.operation.kind === "jssg") {
    return executeJssg(request.operation);
  }
  if (request.operation.kind === "ai") {
    return { ...base(request), status: "failed", error: { message: "no AI executor adapter" } };
  }
  const env = { ...process.env, ...request.operation.env };
  const result = await runner.runCommand(request.operation.command, env); // butterflow_runners::Runner
  return completionFromResult(request.commandId, result);
}
```

`completion_from_result(command_id, result)`

```ts
function completionFromResult(commandId: string, result: Result<string, RunnerError>): OperationCompletion {
  if (result.ok) return { protocolVersion: 2, commandId, status: "succeeded", output: { stdout: result.value } };
  if (result.error.kind === "ShellCommandFailed") {
    return { protocolVersion: 2, commandId, status: "failed",
      error: { message: String(result.error), exitCode: result.error.exitCode, output: result.error.output } };
  }
  // Spawn/wait failures: the bridge cannot tell whether side effects happened.
  return { protocolVersion: 2, commandId, status: "unknown", error: { message: String(result.error) } };
}
```

## JSSG execution

`execute_jssg` in `src/lib.rs`, in order:

1. Resolve `script` against `context.scriptRoot` (or the working directory)
   and canonicalize it; resolve `target.root` beneath the working directory.
2. Build the definition filter: `include`, or `**/*<ext>` for each of the
   language's extensions when `include` is absent (the same
   `get_extensions_for_language` table the workflow engine uses), plus
   `exclude`. Build the invocation filter from `target.include`/`exclude`
   relative to the target root.
3. Enumerate files with `codemod_walk_builder` from `codemod-sandbox`, the
   walker configuration shared with the workflow engine and shard planning:
   hidden files visited, `.gitignore`/`.ignore`/global excludes honored without
   requiring a git repository, symlinks not followed, parent ignore files
   applied. Keep files accepted by both filters; sort.
4. Load the selector (`getSelector`, with no params, as the shipped engine
   does) and build the semantic provider. Workspace mode pre-indexes the
   enumerated set.
5. For each file: read it (skip if it vanished or is not UTF-8, as the engine
   does), run `execute_codemod_with_quickjs` with `params = { input }`, write
   the primary and any `jssgTransform` secondary results beneath the target
   root, refresh the semantic index, and collect `output` when present.
6. Remove the originals of renamed files only now, so a rename cannot delete a
   later enumerated source before it runs (the engine's deferred deletion).

Each file completes one read-transform-write cycle before the next starts.
This intentionally uses a worker budget of one until the shared global file
scheduler and path locks exist.

Legacy JSSG returns (`string | null`) still work and contribute no output
entry. A top-level transform may also return `{ content?, output }`
(`StructuredCodemod` in `@codemod.com/jssg-types`); `content` is applied and
each present `output` is appended to the completion array in sorted file
order. `jssgTransform` accepts only `string | null` transforms: a structured
result from a secondary transform is a runtime error, not a discarded value.

Failure status tracks the bridge's own writes. Every error before the first
write (script resolution, language, globs, target root, selector, semantic
root or indexing, reading, and a transform error on an earlier file) is
`failed` with nothing changed. Once any file has been written, a later error is
`unknown` because earlier files may already differ. A transform that writes
through the curated `fs` module before failing is not tracked.

The local profile grants no optional sandbox capabilities (no `fetch`, real
`fs`, `child_process`, LLM, or shared workflow state); the curated `fs` module
is limited to the target root.

`main` in `src/main.rs`

```ts
function main(argv: string[]): number {
  const [requestPath, responsePath] = argv;
  if (!requestPath || !responsePath) return 2;
  let text: string;
  try { text = readFileSync(requestPath, "utf8"); }
  catch (e) { return writeError(responsePath, "", `failed to read request: ${e}`, 3); }
  let request: OperationRequest;
  try { request = parseRequest(text); }
  catch (e) { return writeError(responsePath, commandIdHint(text), String(e), 3); }
  const completion = await execute(new DirectRunner({ quiet: true }), request);
  try { writeFileSync(responsePath, JSON.stringify(completion)); return 0; }
  catch { return 4; }
}

function writeError(path: string, commandId: string, message: string, code: number): number {
  try { writeFileSync(path, JSON.stringify({ protocolVersion: 2, commandId, status: "failed", error: { message } })); }
  catch {}
  return code;
}
```

`cancelled` is never produced by Rust. The TypeScript `BridgeExecutor` writes
the request file (adding `context.scriptRoot` when configured), spawns the
binary, and reads the response file. If a response exists it is used regardless
of exit code (error completions are structured); otherwise a signal maps to
`cancelled` and any other exit to `unknown`.

## Why Rust here

- `DirectRunner` already implements the shell semantics used by YAML workflows
  (`sh -c`, shebang scripts, env clearing, output capture, exit-code errors).
  Reusing it keeps the prototype's `exec` behavior identical to the production
  engine instead of reimplementing it in Node. Its current returned output is
  platform-dependent: stdout and stderr are combined on Unix, while non-Unix
  builds return stdout for successful commands. A production structured stdio
  contract remains future work.
- The bridge is one-shot JSON over two files, so no N-API or WASM build is
  needed, the TypeScript side stays a plain `child_process.spawn`, and the
  binary builds with `cargo build -p butterflow-execution-bridge` alone,
  independent of the full CLI.
- No orchestration is in Rust: no plans, loops, replay, or history. The bridge's
  JSSG adapter only resolves files, configures semantics, invokes the existing
  sandbox, and applies its file results. AI remains unimplemented.

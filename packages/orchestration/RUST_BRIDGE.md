# Rust execution bridge, explained in TypeScript

All orchestration logic lives in TypeScript. Rust is used for exactly one
thing: running an `exec` operation through the existing
`butterflow_runners::Runner` implementation (`DirectRunner`) so the prototype
reuses the shell execution, environment handling, and output capture that the
YAML engine already has. The whole bridge is equivalent to:

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
  | { kind: "jssg"; package: string; target?: Target; input?: Json } // decoded only, never executed here
  | { kind: "ai"; prompt: string; input?: Json }; // decoded only, never executed here

// Only jssg carries a file selection; a future JSSG adapter enforces it.
interface Target { root?: string; include?: string[]; exclude?: string[] }
```

`OperationRequest` / `OperationCompletion` / `CompletionError` /
`CompletionStatus`

```ts
interface OperationRequest { protocolVersion: 1; commandId: string; operation: Operation }
type CompletionStatus = "succeeded" | "failed" | "cancelled" | "unknown";
interface CompletionError { message: string; exitCode?: number; output?: string }
interface OperationCompletion {
  protocolVersion: 1; commandId: string; status: CompletionStatus;
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
  if (request.protocolVersion !== 1) throw new Error("unsupported protocol version");
  return request;
}
```

`execute(runner, request)`

```ts
async function execute(runner: Runner, request: OperationRequest): Promise<OperationCompletion> {
  if (request.operation.kind !== "exec") {
    return { ...base(request), status: "failed", error: { message: `no executor adapter for '${kind}'` } };
  }
  const env = { ...process.env, ...request.operation.env };
  const result = await runner.runCommand(request.operation.command, env); // butterflow_runners::Runner
  return completionFromResult(request.commandId, result);
}
```

`completion_from_result(command_id, result)`

```ts
function completionFromResult(commandId: string, result: Result<string, RunnerError>): OperationCompletion {
  if (result.ok) return { protocolVersion: 1, commandId, status: "succeeded", output: { stdout: result.value } };
  if (result.error.kind === "ShellCommandFailed") {
    return { protocolVersion: 1, commandId, status: "failed",
      error: { message: String(result.error), exitCode: result.error.exitCode, output: result.error.output } };
  }
  // Spawn/wait failures: the bridge cannot tell whether side effects happened.
  return { protocolVersion: 1, commandId, status: "unknown", error: { message: String(result.error) } };
}
```

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
  try { writeFileSync(path, JSON.stringify({ protocolVersion: 1, commandId, status: "failed", error: { message } })); }
  catch {}
  return code;
}
```

`cancelled` is never produced by Rust. The TypeScript `BridgeExecutor` writes
the request file, spawns the binary, and reads the response file. If a response
exists it is used regardless of exit code (error completions are structured);
otherwise a signal maps to `cancelled` and any other exit to `unknown`.

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
- Nothing else is in Rust: no plans, loops, replay, history, scheduling, JSSG,
  or AI. Those are TypeScript and can move behind the same JSON seams later.
  The JSSG `target` is decoded here for protocol parity only; no file set is
  resolved or enforced until a JSSG adapter exists.

# Rust execution bridge, explained in TypeScript

The Rust side of the prototype is `crates/execution-bridge`, one binary with
two modes. Neither mode walks a repository, interprets globs, orders files,
applies edits, defers deletions, aggregates outputs, or decides
repository-level failure policy. All of that lives in
`packages/orchestration/src` (`walker.ts`, `gitignore.ts`, `staging.ts`,
`jssg.ts`). Rust owns execution and the security checks on its own side of
the boundary.

```text
exec   TypeScript BridgeExecutor -> one-shot file protocol -> butterflow_runners::DirectRunner
jssg   TypeScript executeJssg    -> JSONL worker (--jssg-worker) -> JssgSession -> QuickJS sandbox
```

## Files

- `crates/execution-bridge/src/lib.rs`: serde structs mirroring
  `packages/orchestration/src/protocol.ts` (`OperationRequest`,
  `OperationCompletion`, protocol version 3), `parse_request`, `execute` for
  `exec`, and `validate_relative_path`. `jssg` and `ai` requests are refused
  by the one-shot path.
- `crates/execution-bridge/src/worker.rs`: the JSONL message types
  (`WorkerRequest`, `WorkerResponse`) and `run_worker`, the loop that owns one
  session for the life of the process.
- `crates/execution-bridge/src/session.rs`: `JssgSession`: a resolved script,
  its `OxcResolver`, the selector loaded once, the language, the invocation
  input, and an optional `LazySemanticProvider` shared by every file.
- `crates/execution-bridge/src/paths.rs`: containment checks for every path
  that crosses the boundary in either direction.
- `crates/execution-bridge/src/main.rs`: `butterflow-execution-bridge
  <request.json> <response.json>` (one-shot) and `butterflow-execution-bridge
  --jssg-worker` (JSONL on stdin/stdout).
- Tests: `tests/protocol.rs` (fixtures in `fixtures/protocol`), `tests/worker.rs`
  (the loop over in-memory streams), `tests/session.rs`, `tests/paths.rs`,
  `tests/bin.rs` (both modes over real pipes and files), and
  `tests/walker_parity.rs` (the engine side of `fixtures/walker/cases.json`).

## Worker protocol (JSONL, version 3)

One JSON object per line in each direction, strictly request/response in
order. Every message denies unknown fields on both sides
(`deny_unknown_fields` in Rust, `isWorkerRequest` / `isWorkerResponse` in
`worker-protocol.ts`). The TypeScript side also refuses any path in a response
that is not a safe relative path before it looks at the content.

```ts
type WorkerRequest =
  | {
      type: "open";
      protocolVersion: 3;
      script: string; // safe relative path, resolved beneath scriptRoot
      scriptRoot: string; // absolute; executor context, never command identity
      language: string;
      targetRoot: string; // absolute; every later path is relative to it
      semanticAnalysis?: "file" | "workspace" | { mode: "file" | "workspace"; root?: string };
      input?: Json; // reaches the transform as options.params.input
    }
  | { type: "index"; path: string; content: string }
  | { type: "transform"; path: string; content: string }
  | { type: "close" };

type FileResult =
  | { kind: "modified"; content: string; renameTo?: string } // renameTo is target-root-relative
  | { kind: "unmodified" }
  | { kind: "skipped" }; // the selector matched nothing

type WorkerResponse =
  | { type: "opened"; protocolVersion: 3; extensions: string[]; semanticMode: "file" | "workspace" | null }
  | { type: "indexed" }
  | {
      type: "transformed";
      result: { primary: FileResult; secondary: { path: string; result: FileResult }[]; output?: Json };
    }
  | { type: "closed" }
  | { type: "error"; message: string; fatal: boolean };
```

- `open` resolves the script, loads the selector (`getSelector`, with no
  params, as the shipped engine does), builds the semantic provider, and
  answers with the language's file extensions from
  `get_extensions_for_language`. TypeScript derives the definition's default
  include globs from that list, so there is no second extension table. A
  failed `open` is a fatal error and the worker exits.
- `index` feeds one file into the semantic provider
  (`notify_file_processed`). It is a no-op without a provider.
- `transform` runs `execute_codemod_with_quickjs` for one file with
  `stage_writes: true` and returns the result as data. A transform error is
  a recoverable `error` (`fatal: false`): the session stays open and the
  host decides what to do (it fails the command).
- `close` answers `closed` and exits 0. EOF on stdin also exits 0, so a host
  that dies takes its worker with it; the host kills the worker with
  `SIGKILL` on abort.
- Exit codes of the worker mode: 0 closed or EOF, 3 malformed message or
  protocol misuse (an `error` line was written first), 4 I/O failure.

The pipes are the protocol channel owned by the spawning host. The sandbox's
`console` goes to runtime events, never to the process streams, and nothing
else in the crate writes to them.

### One session, one runtime configuration

`JssgSession` holds everything that is constant for a command: script path,
module resolver, selector, language, params, target root, semantic provider.
Each `transform` still creates a fresh QuickJS runtime for the file, exactly
as the workflow engine does per file; the session is what is shared, not the
JavaScript heap.

### What `stage_writes` changes in the sandbox

`SgRoot.write()` on a root obtained from `definition()` or `references()`
writes straight to disk in the workflow engine. With
`JssgExecutionOptions::stage_writes` the same call validates the path against
the target directory and records a `FileChange` in the secondary results
instead. The session always sets it; the engine, CLI, and MCP callers pass
`false`, so their behavior is unchanged (`crates/codemod-sandbox/tests`
covers both).

## Security model

Every path is validated independently on both sides. Neither side trusts the
other's check.

Rust (`paths.rs`):

- `index.path` and `transform.path` must be safe relative paths (non-empty,
  not absolute on any platform including `C:\` and `\\server` forms, no `..`
  segment). The path is joined to the canonical target root, its nearest
  existing ancestor (or the file itself) is canonicalized, and the result
  must stay inside the root. A symlinked file or directory pointing outside
  is rejected; the resolved real path is what the sandbox sees.
- `rename_to`, `jssgTransform` targets, and staged `write()` targets may be
  absolute or relative. `..` components are rejected, the same nearest
  existing ancestor check applies, and the result is normalized to a
  root-relative `/`-separated path. Components containing `\` are refused
  because the TypeScript side splits on both separators. The sandbox's own
  `validate_path_within_target` runs before any of this; the session check is
  the second, independent gate.
- `open` requires absolute `scriptRoot` and `targetRoot`, a safe relative
  `script`, and a `semanticAnalysis.root` that resolves inside the target root.

TypeScript (`paths.ts`, `staging.ts`, `jssg.ts`):

- Response validation rejects any path that is not a safe relative path.
- `resolveInsideRoot` proves every edited path, rename target, and source
  path lies beneath the real target root through its nearest existing
  ancestor, rejecting symlinks that leave the root and dangling symlinks.
  It runs when a result is staged and again immediately before each write
  at commit.
- The target root itself must resolve beneath the working directory the
  same way, so a target root that is a symlink out of the repository fails
  before any worker starts.

The local profile grants no optional sandbox capabilities (no `fetch`, real
`fs`, `child_process`, LLM, or shared workflow state); the curated `fs`
module is limited to the target root. A transform that writes through the
curated `fs` module is outside the staging model and is not tracked.

## Transaction semantics (TypeScript)

`executeJssg` in `jssg.ts` runs one command:

1. Resolve the target root beneath the working directory.
2. Spawn one worker and `open` it.
3. Select files with `selectFiles` (engine walker semantics; see the README).
4. In workspace semantic mode, `index` every selected file first. This is
   the engine's `pre_index_workspace_semantics` set: the effective selected
   set, not everything under the semantic root. The provider still resolves
   imports through the semantic root lazily, as it does in the engine.
5. For each file in order: read it (a staged edit from an earlier secondary
   result is used instead of disk, and a file renamed away earlier is
   skipped), `transform`, stage the result, and `index` every staged write
   so later files observe earlier edits in the semantic index, exactly as the
   engine notifies the provider after each write. Disk reads inside the
   sandbox (`jssgTransform`, curated `fs`) still see the pre-commit snapshot.
6. `close` the worker, then commit.

Staging rules (`staging.ts`): a later file's own primary result supersedes
an earlier secondary edit of the same path (chained); any other second
write to one destination is a conflict; a rename source may be renamed once;
a rename destination may not be an existing file unless that file was itself
renamed away. Conflicts fail the command before anything is written.

Commit: every destination is written through a sibling temp file plus an
atomic rename (mode preserved), then rename sources are removed. Each file is
atomic; the set is not. A failure or abort part-way returns `unknown` with
`{ phase: "commit", applied, failed, remaining, aborted }`.

Failure classification:

| status      | when                                                    | repository |
| ----------- | ------------------------------------------------------- | ---------- |
| `failed`    | open, select, index, transform, stage, or worker death  | unchanged  |
| `cancelled` | the signal fired before commit                          | unchanged  |
| `unknown`   | commit stopped part-way (error or abort during commit)  | partial    |
| `succeeded` | every staged write and deletion applied                 | committed  |

`error.details.phase` names the phase, with `path` for file-level failures.

## One-shot file protocol (exec)

Unchanged in shape from protocol 2 apart from the version and the optional
`error.details`:

```ts
interface OperationRequest {
  protocolVersion: 3; commandId: string; operation: Operation; context?: { scriptRoot?: string };
}
interface OperationCompletion {
  protocolVersion: 3; commandId: string; status: "succeeded" | "failed" | "cancelled" | "unknown";
  output?: Json; error?: { message: string; exitCode?: number; output?: string; details?: Json };
}
```

`execute` runs `exec` through `DirectRunner` (`sh -c`, combined stdout and
stderr on Unix) and refuses `jssg` ("runs through the worker protocol") and
`ai` ("no executor adapter"). Exit codes: 0 completion written, 2 wrong
arguments, 3 unreadable or malformed request, 4 response could not be written.
`BridgeExecutor` kills the process on abort and reports `cancelled` when the
kill landed before a response, `unknown` otherwise.

## Why this shape

- Rust keeps what only Rust can do: the QuickJS sandbox, ast-grep, module
  resolution, the semantic providers, and the language table. It exposes
  them through one stateful session so a command loads its script and
  selector once and indexes its workspace once.
- Everything that is policy (which files, in what order, what counts as a
  conflict, when to write, how to classify a failure) is TypeScript, where
  the authoring model lives and iterates quickly. The Rust side cannot write
  repository files on this path even if asked.
- JSONL over the child's own pipes keeps one process per command, no N-API
  or WASM build, and plain `child_process.spawn` on the TypeScript side.

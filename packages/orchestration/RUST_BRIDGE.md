# Rust execution bridge, explained in TypeScript

The Rust side of the prototype is `crates/execution-bridge`: one binary, one
exchange. It reads an `OperationRequest` from a file, executes it, and writes
an `OperationCompletion` to another file. It never reads author files, walks
a repository, interprets globs, orders files, applies edits, aggregates
outputs, or decides repository-level failure policy; all of that is
`packages/orchestration/src` (`build.ts`, `files.ts`, `jssg.ts`). Rust owns
execution and the checks on its own side of the boundary.

```text
exec   TypeScript BridgeExecutor -> bridge process -> butterflow_runners::DirectRunner
jssg   TypeScript executeJssg    -> bridge process -> one batch through the QuickJS sandbox
```

## Files

- `crates/execution-bridge/src/lib.rs`: serde structs mirroring
  `packages/orchestration/src/protocol.ts` (protocol version 5),
  `parse_request`, `execute` (`exec` through the runner, `jssg` through
  `jssg::transform_batch`, `ai` refused).
- `crates/execution-bridge/src/jssg.rs`: one batch (verified artifact,
  language, static selector, input, optional semantic provider, then every
  file in order) and the path containment applied on both directions.
- `crates/execution-bridge/src/main.rs`: `butterflow-execution-bridge
  <request.json> <response.json>`. Exit codes: 0 completion written, 2 wrong
  arguments, 3 unreadable or malformed request (an error completion is still
  written), 4 runtime failure. Files are the channel because non-CLI crates
  must not write to the process streams.
- Tests: `tests/protocol.rs` (the shared `fixtures/protocol` and strictness),
  `tests/jssg.rs` (batches through the real sandbox), `tests/bin.rs`, and
  `tests/contracts.rs` (the engine side of `fixtures/walker/cases.json` and of
  `src/languages.json`).
- In `crates/codemod-sandbox`, the primitives the bridge uses and the engine
  does not: `execute_codemod_with_loader` (the existing
  `execute_codemod_with_quickjs` with the module loader as a parameter, so a
  bundle held in memory runs through an `InMemoryResolver` and
  `InMemoryLoader` under a virtual module name), `selector_from_value` and
  `selector_matches` (a `RuleConfig` from static rule data and the
  eligibility test, without a JavaScript runtime), and the `stage_writes`
  option.

## Protocol (JSON files, version 5)

```ts
interface OperationRequest {
  protocolVersion: 5;
  commandId: string;
  operation: ExecOperation | JssgOperation | AiOperation; // command identity, recorded in history
  context?: {
    targetRoot?: string; // absolute; every file path below is relative to it
    files?: { path: string; content: string }[]; // the jssg batch, in transform order
    artifact?: { source: string }; // the bundled transform named by operation.transform
  };
}

interface JssgOperation {
  kind: "jssg";
  transform: { name: string; hash: string }; // artifact identity: name + SHA-256 of the source
  language: string;
  include?: string[];
  exclude?: string[];
  semanticAnalysis?: "file" | "workspace" | { mode: "file" | "workspace"; root?: string };
  selector?: { rule: Json; constraints?: Json; utils?: Json }; // static prefilter
  target?: { root?: string; include?: string[]; exclude?: string[] };
  input?: Json;
}

interface OperationCompletion {
  protocolVersion: 5;
  commandId: string;
  status: "succeeded" | "failed" | "cancelled" | "unknown";
  output?: Json; // exec: { stdout }; jssg: { files: FileOutcome[] }
  error?: { message: string; exitCode?: number; output?: string; details?: Json };
}

interface FileOutcome {
  path: string; // the batch file
  edits: { path: string; content: string; renameTo?: string }[]; // target-root-relative
  output?: Json; // from a StructuredCodemod return; absent for skipped files
}
```

Every struct denies unknown fields on both sides (`deny_unknown_fields` in
Rust, the `is*` guards in `protocol.ts`), so a `target` on `exec` or `ai`, a
`script` path, a selector `id` or `language`, or a stray field in the
context is a parse error rather than a dropped field. `context` is
executor-side data: it is attached by the host that spawns the bridge and
never enters the command record that replay compares. The artifact source
lives only there; the operation carries its name and hash, which is why the
recorded command is the same on every checkout and changes when the
transform or a bundled helper changes.

### A jssg batch

`transform_batch` sets up once: canonical target root, the artifact
(`context.artifact.source` must be present and its SHA-256 must equal
`operation.transform.hash`, which must be lowercase hex), the language, the
selector (`selector_from_value` with `id: "selector"` and the operation's
language), and the semantic provider (`LazySemanticProvider`, file or
workspace scope). The source is registered with an `InMemoryResolver` under
`<name>.jssg.js` (non-alphanumeric characters of the name replaced), the
module name sandbox errors mention. Then:

1. every `files[].path` is validated (see below) before anything runs;
2. in workspace mode, every batch file is fed to the provider
   (`notify_file_processed`): the whole selected set, including files the
   selector will skip, so a matching transform can resolve definitions and
   references in them;
3. a file the selector does not match (`selector_matches` on the content,
   parsed once natively) is recorded with no edits and no output and never
   reaches the sandbox;
4. each remaining file runs through `execute_codemod_with_loader` with the
   in-memory loader and `stage_writes: true`, from the content the host
   supplied; no `selector_config` is passed, so `options.matches` is
   undefined and the transform selects nodes itself;
5. the primary result, `jssgTransform` results, and staged `write()` results
   become `edits`; `Unmodified` and `Skipped` produce no edit.

Snapshot semantics: no transform sees another transform's edits, and the
index is not rebuilt between files (the sandbox's own `write()` does refresh
the entry it edits). The workflow engine is incremental instead: it writes
each file before moving on and later files read it from disk. This is the
one behavioral difference on the JSSG path and is why TypeScript treats two
edits of one destination as a conflict rather than chaining them.

The first transform error fails the whole batch; nothing is returned for
earlier files because nothing would be written anyway. Each `transform`
still creates a fresh QuickJS runtime for its file, exactly as the engine
does; what is shared is the configuration, the module source, and the
provider, not the JavaScript heap.

The artifact is self-contained (esbuild inlined every helper), so the bridge
has no module resolver, no tsconfig discovery, and no transpiler on this
path: the only imports left in an artifact are `codemod:*` modules and the
sandbox's built-ins, which the sandbox's own resolver serves. A legacy
`getSelector` export in an artifact is ignored; Butterflow, `codemod jssg
run`, and `jssg list-applicable` keep executing `getSelector` exactly as
before through the unchanged `extract_selector_with_quickjs`.

### What `stage_writes` changes in the sandbox

`SgRoot.write()` on a root obtained from `definition()` or `references()`
writes straight to disk in the workflow engine. With
`JssgExecutionOptions::stage_writes` the same call validates the path against
the target directory and records a `FileChange` in the secondary results
instead. The bridge always sets it; the engine, CLI, and MCP callers pass
`false`, so their behavior is unchanged.

## Security model

Every path is validated independently on both sides. Neither side trusts the
other's check.

Rust (`jssg.rs`):

- `targetRoot` must be absolute and canonicalize to a directory;
  `semanticAnalysis.root` must be a safe relative path (non-empty, not
  absolute on any platform including `C:\` and `\\server` forms, no `..`
  segment).
- `transform.name` must be non-empty and `transform.hash` a lowercase hex
  SHA-256 equal to the digest of `context.artifact.source`; a mismatch or a
  missing source fails the batch before any file is touched.
- `selector` must have a `rule` and at most `constraints` and `utils`; the
  rule is compiled by ast-grep before any file runs, so an invalid rule
  fails the batch.
- `files[].path` must be a safe relative path whose nearest existing ancestor
  (or the file itself) canonicalizes inside the target root, so a symlinked
  file or directory pointing outside is rejected; the resolved real path is
  what the sandbox sees.
- `rename_to`, `jssgTransform` targets, and staged `write()` targets may be
  absolute or relative. `..` components are rejected, the same nearest
  existing ancestor check applies, and the result is normalized to a
  root-relative `/`-separated path. The sandbox's own
  `validate_path_within_target` runs before any of this; the bridge check is
  the second, independent gate.

TypeScript (`paths.ts`, `jssg.ts`, `protocol.ts`):

- `isArtifactRef` and `isSelector` reject malformed identities and selector
  data when a definition is created and when a request is validated.
- `isFileOutcomes` rejects any returned path that is not a safe relative path.
- `resolveInsideRoot` proves every edited path and rename target lies beneath
  the real target root through its nearest existing ancestor, rejecting
  symlinks that leave the root and dangling symlinks, before anything is
  staged.
- The target root itself must resolve beneath the working directory the same
  way, so a target root that is a symlink out of the repository fails before
  any bridge starts.

The local profile grants no optional sandbox capabilities (no `fetch`, real
`fs`, `child_process`, LLM, or shared workflow state); the curated `fs`
module is limited to the target root. A transform that writes through the
curated `fs` module is outside the batch result and is not tracked.

## Transaction semantics (TypeScript)

`executeJssg` in `jssg.ts` runs one command:

1. Look up the artifact by `operation.transform.hash` in the executor's
   store; a missing artifact fails in phase `artifact` without spawning.
2. Resolve the target root beneath the working directory, select files
   (`files.ts`, engine walker semantics; see the README), and read them.
   Files that vanished or are not UTF-8 are skipped, as the engine does.
3. Send the artifact source and the batch to one bridge process and wait.
4. Validate the outcomes, merge every edit into one write set, and reject
   what snapshot semantics cannot reconcile: two edits to one destination, a
   source renamed twice, a write to a path another edit renames away, and a
   rename onto a file that exists unless that file is itself renamed away.
5. If the signal has not fired, write every destination (creating
   directories), then remove rename sources.

Nothing touches the repository before step 5. Failure classification:

| status      | when                                                               | repository |
| ----------- | ------------------------------------------------------------------ | ---------- |
| `failed`    | artifact, select, bridge, transform, invalid result, or a conflict | unchanged  |
| `cancelled` | the signal fired before commit (the bridge is SIGKILLed)           | unchanged  |
| `unknown`   | commit stopped part-way                                            | partial    |
| `succeeded` | every write and deletion applied                                   | committed  |

`error.details.phase` is `artifact`, `select`, `transform`, `stage`, or
`commit`; a commit failure also carries `applied`, `failed`, and
`remaining`. Per-file writes are ordinary `writeFileSync` calls; there is no
cross-file transaction on ordinary filesystems.

## Why this shape

- Rust keeps what only Rust can do: the QuickJS sandbox, ast-grep, the
  semantic providers. One process per command loads the artifact once and
  indexes the batch once, with no session state to manage and no author
  filesystem to resolve against.
- Everything that is policy (which files, in what order, what counts as a
  conflict, when to write, how to classify a failure) and everything that is
  build (extraction, bundling, identity) is TypeScript, where the authoring
  model lives and iterates quickly. The Rust side cannot write repository
  files on this path even if asked, and cannot run a transform whose source
  does not match the identity the workflow recorded.
- One request and one response per command means cancellation is one
  `SIGKILL`, and `exec` and `jssg` share the same spawn path (`bridge.ts`).

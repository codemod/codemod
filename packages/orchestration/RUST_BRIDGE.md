# Rust execution bridge, explained in TypeScript

The Rust side of the prototype is `crates/execution-bridge`: one binary, one
exchange. It reads an `OperationRequest` from a file, executes it, and writes
an `OperationCompletion` to another file. It never reads author files, walks
a repository, interprets globs, orders files, applies edits, aggregates
outputs, or decides repository-level failure policy; all of that is
`packages/orchestration/src` (`build.ts`, `files.ts`, `jssg.ts`). Rust owns
execution and the checks on its own side of the boundary.

```text
shell       TypeScript BridgeExecutor -> bridge process -> butterflow_runners::DirectRunner
jssg        TypeScript executeJssg    -> bridge process -> one batch through the QuickJS sandbox
agent       TypeScript BridgeExecutor -> bridge process -> codemod_ai::execute::execute_ai_step (builtin)
                                                        -> `claude` / `codex` CLI process (claude-code, codex)
assessment  TypeScript executeAssessment -> @typesafe-ai/sdk TypeSafeClient.systemOne() (no bridge process)
```

## Files

- `crates/execution-bridge/src/lib.rs`: serde structs mirroring
  `packages/orchestration/src/core/protocol.ts` (protocol version 8),
  `parse_request`, `execute` (`shell` through the runner, `jssg` through
  `jssg::transform_batch`, `agent` through `agent::run`, `assessment` decoded
  for parity and refused: the TypeScript host executes it).
- `crates/execution-bridge/src/agent.rs`: `agent` settings from the engine's
  AI step environment (`LLM_API_KEY` required; `LLM_PROVIDER` default
  `openai`, `LLM_MODEL` default `gpt-4o`, `LLM_BASE_URL` default per
  provider), the task prompt (the operation's prompt, its input as pretty
  JSON, and the JSON reply instruction when `responseFormat` is `json`), and
  dispatch on `backend`: `builtin` is one `execute_ai_step` run in the
  process working directory with exactly the backend's `tools` and
  `maxSteps`; `claude-code` and `codex` go to `external.rs`. For `builtin`,
  `main.rs` calls
  `agent::take_process_settings` before the tokio runtime (and any tool
  process) starts: with `CODEMOD_BRIDGE_SECRETS=stdin` (how the TypeScript
  host launches it) the key is read from stdin as `{"LLM_API_KEY": "..."}`,
  otherwise from the environment, and `LLM_API_KEY` and the marker are
  removed from the process environment either way. The library `execute()`
  reads the key from the environment and does not remove it. For external
  backends `main.rs` only removes the key and marker (`scrub_process_env`), and
  CLI processes are started with both removed even on the library path.
  Output `{ text }`; failures are `failed` with
  `details: { phase: "config" | "execute", repositoryMayBeModified }`.
- `crates/execution-bridge/src/external.rs`: the Claude Code and Codex
  harnesses. Lookup on absolute `PATH` entries only; a private directory per
  run holding an empty login-check directory and the CLI's `TMPDIR`; one
  `Launch` (credential-looking variables removed per `is_secret_env_name`,
  `TMPDIR`/`TMP`/`TEMP` set) shared by the login check (30 s timeout, run from
  the empty directory; only `claude auth status --json`'s `loggedIn` or
  `codex login status`'s exit code is used) and the task; argv built from
  `claude --help` / `codex exec --help` with no bypass flag
  (`FORBIDDEN_FLAG_FRAGMENTS` is asserted in tests), including Codex's
  `shell_environment_policy.inherit="core"` and
  `sandbox_workspace_write.exclude_slash_tmp=true`; the prompt on stdin;
  pipes read by background tasks, with at most `PIPE_DRAIN_GRACE` (2 s) of
  further reading once the CLI exits so a descendant holding stdout cannot
  hang the bridge; Claude's stdout kept up to 16 MiB and the final text taken
  from its `{"type":"result"}` object; Codex's `--json` stream scanned line by
  line (lines over 16 MiB skipped) keeping only the last `agent_message` text
  and the last error message; the stderr tail up to 8 KiB. Missing CLI, logged out, a failed
  or timed-out login check, a spawn failure, or (Codex) no git repository are
  `config` failures; anything after the task process starts is `execute`
  with `repositoryMayBeModified: true`. The child is killed if the bridge
  drops it; host cancellation kills the bridge's process tree as for any
  bridge. This is deliberately separate from `butterflow-core`'s
  `ai_handoff.rs` / `ai_agent_stream.rs`, which launch interactive handoffs
  with bypass flags and normalize progress streams for the TUI; the only
  logic in common is a PATH lookup, and depending on `butterflow-core` would
  invert the bridge's crate boundary.
- `crates/execution-bridge/src/jssg.rs`: one batch (verified artifact,
  language, static selector, input, optional semantic provider, then every
  file in order) and the path containment applied on both directions.
- `crates/execution-bridge/src/main.rs`: `butterflow-execution-bridge
  <request.json> <response.json>`. The request must be a regular file (not a
  symlink); the response is created with `create_new` (`O_CREAT|O_EXCL`,
  mode 0600) and never opened if something exists at the path. Exit codes: 0 completion written, 2 wrong
  arguments, 3 unreadable or malformed request (an error completion is still
  written), 4 runtime failure. Files are the channel because non-CLI crates
  must not write to the process streams.
- Tests: `tests/protocol.rs` (the shared `fixtures/protocol` and strictness),
  `tests/jssg.rs` (batches through the real sandbox), `tests/bin.rs`, and
  `tests/contracts.rs` (the engine side of `fixtures/walker/cases.json` and of
  `src/execution/languages.json`).
- In `crates/codemod-sandbox`, the primitives the bridge uses and the engine
  does not: `execute_codemod_with_loader` (the existing
  `execute_codemod_with_quickjs` with the module loader as a parameter, so a
  bundle held in memory runs through an `InMemoryResolver` and
  `InMemoryLoader` under a virtual module name), `selector_from_value` and
  `selector_matches` (a `RuleConfig` from static rule data and the
  eligibility test, without a JavaScript runtime), and the `stage_writes`
  option.

## Protocol (JSON files, version 8)

```ts
interface OperationRequest {
  protocolVersion: 8;
  commandId: string;
  // command identity, recorded in history
  operation: ShellOperation | JssgOperation | AgentOperation | AssessmentOperation;
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

interface AgentOperation {
  kind: "agent";
  prompt: string;
  input?: Json;
  // always present; each variant allows only its own settings
  backend:
    | {
        kind: "builtin";
        // codemod-ai tool names; `[]` is a tool-less agent
        tools: ("bash" | "str_replace_based_edit_tool" | "json_edit_tool" | "glob"
          | "sequentialthinking" | "task_done" | "ckg_tool" | "mcp_tool")[];
        maxSteps?: number; // positive integer
      }
    | { kind: "claude-code"; tools: ("Read" | "Edit" | "Write" | "Glob" | "Grep" | "Bash")[] }
    | { kind: "codex"; sandbox: "read-only" | "workspace-write" };
  responseFormat?: "json";
}

interface AssessmentOperation {
  kind: "assessment"; // never executed by the bridge
  state: string | Json[] | { [key: string]: Json };
  questions: {
    [id: string]:
      | { type: "noul"; instructions: Json; criteria?: { true?: Json; false?: Json } }
      | { type: "choice"; instructions: Json; criteria: { [option: string]: Json } }
      | { type: "score"; instructions: Json; criteria: Json[] };
  };
  model?: string;
}

interface OperationCompletion {
  protocolVersion: 8;
  commandId: string;
  status: "succeeded" | "failed" | "cancelled" | "unknown";
  // shell: { stdout }; jssg: { files: FileOutcome[] }; agent: { text };
  // assessment: { model, answers, usage: { inputTokens, outputTokens } }
  output?: Json;
  error?: { message: string; exitCode?: number; output?: string; details?: Json };
}

interface FileOutcome {
  path: string; // the batch file
  edits: { path: string; content: string; renameTo?: string }[]; // target-root-relative
  output?: Json; // from a StructuredCodemod return; absent for skipped files
}
```

Every struct denies unknown fields on both sides (`deny_unknown_fields` in
Rust, the `is*` guards in `protocol.ts`), so a `target` on `shell`, `agent`, or `assessment`, an unknown question `type`, a
`script` path, a selector `id` or `language`, or a stray field in the
context is a parse error rather than a dropped field. `context` is
executor-side data: it is attached by the host that spawns the bridge and
never enters the command record that replay compares. The artifact source
lives only there; the operation carries its name and hash, which is why the
recorded command is the same on every checkout and changes when the
transform or a bundled helper changes.

TypeScript validation is authoritative. `isOperation`, `questionsProblem`,
`assessmentResultProblem`, and the runnable constructors in
`packages/orchestration/src` decide what a valid command and a valid result
are, and every command is validated there before it reaches any executor.
The Rust structs mirror the shape (field names, required fields, variant
names, tool names, `responseFormat`) so a malformed request is still a parse
error, but they do not re-check content rules: non-empty or reserved
question ids, at least two choice options or score levels, non-blank
instructions and state, duplicate tool names (checked when the agent runs,
as a `config` failure), or answer ranges. The bridge never executes
`assessment` and never validates assessment answers.

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
  `SIGKILL`, and `shell` and `jssg` share the same spawn path (`bridge.ts`).

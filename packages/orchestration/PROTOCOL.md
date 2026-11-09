# The operation protocol, and how the runtimes consume it

`src/core/protocol.ts` (protocol version 8) is the one wire contract between
the control plane and whatever executes an operation: an `OperationRequest`
goes in, an `OperationCompletion` comes out. The runtime, `quickjs` (see
`RUNTIMES.md`), executes every operation kind against it.
The control plane never reads author files, and an executor never walks a
repository, interprets globs, orders files, applies edits, aggregates outputs,
or decides repository-level failure policy; all of that is
`packages/orchestration/src` (`build.ts`, `files.ts`, `jssg.ts`).

```text
shell       executeShell (shell.ts) over Host.spawn                    in-process, DirectRunner semantics
jssg        executeJssgCommand (jssg.ts) -> shared batch loop           in-process (QuickJS host)
              (jssg-batch.ts) -> crates/jssg-batch BatchPlan
agent       executeAgent (agent.ts) -> Host.spawn codemod-agent         one runner process per operation
              -> codemod_ai::execute::execute_ai_step (builtin)
              -> `claude` / `codex` CLI process (claude-code, codex)
assessment  executeAssessmentOperation (assessment.ts)                  in-process, one HTTP request per file, up to fileConcurrency at once
              -> decision/{systemone,openai}.ts -> decision/transport.ts
```

Until 2026-11-08 a third runtime, `bridge`, ran `shell`, `jssg`, and `agent`
in one Rust process per command (`crates/execution-bridge`), exchanging the
same JSON through request and response files. It was removed; the protocol,
the JSSG batch rules (now `crates/jssg-batch`), and the agent runner (now
`crates/orchestration-agent`) are what remains of it. A `node` runtime (the
workflow in one Node process, JSSG over a napi addon, `crates/jssg-napi`) was
removed after it.

## Files

- `src/core/protocol.ts`: the wire types, `PROTOCOL_VERSION`, and the `is*`
  guards and `parseCompletion` that validate them.
- `fixtures/protocol/*.json`: shared conformance fixtures, checked by
  `tests/protocol.test.ts` and, for the agent fixtures, by
  `crates/orchestration-agent/tests/protocol.rs`.
- `crates/jssg-batch`: the JSSG batch rules of the runtime (`BatchPlan`:
  planning, containment, workspace index, selector skip, parse, result
  conversion) and the serde mirror of the batch's protocol data (`ArtifactRef`,
  `Selector`, `SemanticAnalysis`, `BatchFile`, `Edit`, `FileOutcome`). Used by
  the QuickJS host (`crates/orchestration-quickjs-host`), in the bundle's
  context and in each transform pool worker. `tests/contracts.rs` is the engine
  side of `fixtures/walker/cases.json` and of `src/execution/languages.json`.
- `crates/orchestration-agent`: the `codemod-agent` binary (below).
  - `src/lib.rs`: serde structs mirroring `protocol.ts`, restricted to
    `kind: "agent"`; any other kind is a parse error.
  - `src/agent.rs`: `agent` settings from the engine's AI step environment
    (`LLM_API_KEY` required; `LLM_PROVIDER` default `openai`, `LLM_MODEL`
    default `gpt-4o`, `LLM_BASE_URL` default per provider), the task prompt
    (the operation's prompt, its input as pretty JSON, and the JSON reply
    instruction when `responseFormat` is `json`), and dispatch on `backend`:
    `builtin` is one `execute_ai_step` run in the process working directory
    with exactly the backend's `tools` and `maxSteps`; `claude-code` and
    `codex` go to `external.rs`. For `builtin`, `main.rs` calls
    `agent::take_process_settings` before the tokio runtime (and any tool
    process) starts: with `CODEMOD_AGENT_SECRETS=stdin` (how the TypeScript
    launcher starts it) the key is read from stdin as `{"LLM_API_KEY": "..."}`,
    otherwise from the environment, and `LLM_API_KEY` and the marker are
    removed from the process environment either way. For external backends
    `main.rs` only removes the key and marker (`scrub_process_env`), and CLI
    processes are started with both removed. Output `{ text }`; failures are
    `failed` with `details: { phase: "config" | "execute",
    repositoryMayBeModified }`.
  - `src/external.rs`: the Claude Code and Codex harnesses. Lookup on
    absolute `PATH` entries only; a private directory per run holding an empty
    login-check directory and the CLI's `TMPDIR`; one `Launch`
    (credential-looking variables removed per `is_secret_env_name`,
    `TMPDIR`/`TMP`/`TEMP` set) shared by the login check (30 s timeout, run
    from the empty directory; only `claude auth status --json`'s `loggedIn` or
    `codex login status`'s exit code is used) and the task; argv built from
    `claude --help` / `codex exec --help` with no bypass flag
    (`FORBIDDEN_FLAG_FRAGMENTS` is asserted in tests), including Codex's
    `shell_environment_policy.inherit="core"` and
    `sandbox_workspace_write.exclude_slash_tmp=true`; the prompt on stdin;
    pipes read by background tasks, with at most `PIPE_DRAIN_GRACE` (2 s) of
    further reading once the CLI exits so a descendant holding stdout cannot
    hang the runner; Claude's stdout kept up to 16 MiB and the final text
    taken from its `{"type":"result"}` object; Codex's `--json` stream scanned
    line by line (lines over 16 MiB skipped) keeping only the last
    `agent_message` text and the last error message; the stderr tail up to
    8 KiB. Missing CLI, logged out, a failed or timed-out login check, a spawn
    failure, or (Codex) no git repository are `config` failures; anything
    after the task process starts is `execute` with
    `repositoryMayBeModified: true`. The child is killed if the runner drops
    it; host cancellation kills the runner's process tree. This is
    deliberately separate from `butterflow-core`'s `ai_handoff.rs` /
    `ai_agent_stream.rs`, which launch interactive handoffs with bypass flags
    and normalize progress streams for the TUI; the only logic in common is a
    PATH lookup.
  - `src/main.rs`: the process wiring (below).
  - Tests: `tests/protocol.rs` (the shared agent fixtures and strictness),
    `tests/bin.rs` (exit codes, the key channel, exactly one completion on
    stdout), `tests/agent_env.rs`, `tests/external.rs` (fake `claude` and
    `codex` executables).
- `src/execution/agent.ts` (`executeAgent`) and `src/execution/agent-env.ts`
  (`agentLaunch`): the portable launcher; `tests/agent-runner.test.ts` runs it
  against `tests/fixtures/fake-agent.mjs`.
- In `crates/codemod-sandbox`, the primitives the batch rules use and the
  engine does not: `selector_from_value` and `selector_matches` (a
  `RuleConfig` from static rule data and the eligibility test, without a
  JavaScript runtime), `classify_transform_result`, and the `stage_writes`
  option.

## Protocol (JSON, version 8)

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

interface ShellOperation {
  kind: "shell";
  command: string;
  env?: Record<string, string>;
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
  kind: "assessment";
  state: string | Json[] | { [key: string]: Json };
  questions: {
    [id: string]:
      | { type: "yesno"; instructions: Json; criteria?: { yes?: Json; no?: Json } }
      | { type: "choice"; instructions: Json; criteria: { [option: string]: Json } }
      | { type: "score"; instructions: Json; criteria: Json[] };
  };
  provider: "systemone" | "openai"; // resolved from the environment when the command is created
  model: string; // likewise; a pinned model option wins
}

interface OperationCompletion {
  protocolVersion: 8;
  commandId: string;
  status: "succeeded" | "failed" | "cancelled" | "unknown";
  // shell: { stdout }; jssg: the per-file structured outputs in file order
  // (a batch dispatch returns { files: FileOutcome[] }); agent: { text };
  // assessment: { provider, model, calibrated, answers, usage: { inputTokens?, outputTokens? } },
  // where any answer may be { type: "refused" }
  output?: Json;
  error?: { message: string; exitCode?: number; output?: string; details?: Json };
}

interface FileOutcome {
  path: string; // the batch file
  edits: { path: string; content: string; renameTo?: string }[]; // target-root-relative
  output?: Json; // from a StructuredCodemod return; absent for skipped files
}
```

Every struct denies unknown fields on both sides (the `is*` guards in
`protocol.ts`, `deny_unknown_fields` in the Rust mirrors), so a `target` on
`shell`, `agent`, or `assessment`, an unknown question `type`, a `script`
path, a selector `id` or `language`, or a stray field in the context is a
parse error rather than a dropped field. `context` is executor-side data: it
is attached by the executor that runs the operation and never enters the
command record that replay compares. The artifact source lives only there;
the operation carries its name and hash, which is why the recorded command is
the same on every checkout and changes when the transform or a bundled helper
changes.

TypeScript validation is authoritative. `isOperation`, `questionsProblem`,
`assessmentResultProblem`, and the runnable constructors in
`packages/orchestration/src` decide what a valid command and a valid result
are, and every command is validated there before it reaches any executor.
The Rust structs mirror the shape (field names, required fields, variant
names, tool names, `responseFormat`) so a malformed request is still a parse
error, but they do not re-check content rules: non-empty or reserved question
ids, at least two choice options or score levels, non-blank instructions and
state, duplicate tool names (checked when the agent runs, as a `config`
failure), or answer ranges.

## How each kind is executed

- **`shell`**: `executeShell` in the control plane's own process (the QuickJS
  host; Node under the test harness), over `Host.spawn`, with the semantics of the workflow
  engine's `butterflow_runners::DirectRunner` (the environment is the host's
  plus the runtime's `env` plus the operation's `env`; `#!/` scripts run
  directly, anything else as `sh -c`; one merged output stream; abort kills
  the process tree).
- **`jssg`**: `executeJssgCommand` builds the request (artifact, target root,
  selected files with their content) and hands it to the runtime's dispatch,
  the shared batch loop `src/execution/jssg-batch.ts`, in the same JS realm as
  the workflow. The batch rules are Rust, reached through the QuickJS host's
  primitives (`jssgBegin`, `jssgIndex`, `jssgParse`, `jssgCall`, `jssgFinish`,
  and `jssgTransformRange` for the transform pool). No process is spawned.
  See "A jssg batch".
- **`agent`**: one `codemod-agent` process per operation. See "The agent
  runner".
- **`assessment`**: `executeAssessmentOperation(options, cwd, request,
  signal)` in the control plane's process: one request per file to the
  decision API the environment names (`src/core/decision.ts`), through the
  recorded `provider`'s adapter (`src/execution/decision/`) and this package's
  transport (timeout, retries, abort). Keys and endpoints come from the
  executor's environment, never from the operation. File-oriented assessments
  read their files through the `Host`. The QuickJS context has no global
  `fetch`; the executor gets an adapter (`src/host/quickjs/fetch.ts`) over the
  host function `httpRequest` (`crates/orchestration-quickjs-host/src/http.rs`),
  which only reaches the decision API root (see "Security model"). An assessment cannot change the
  repository: failures are `failed`, aborts `cancelled`, never `unknown`.

### A jssg batch

The loop opens the batch (`BatchPlan::for_paths(.., Containment::PerFile)`):
canonical target root, the artifact (`context.artifact.source` must be
present and its SHA-256 must equal `operation.transform.hash`, which must be
lowercase hex), the language, the selector (`selector_from_value` with
`id: "selector"` and the operation's language), every `files[].path` checked
as a safe relative path, and the semantic provider
(`semantic_factory::LazySemanticProvider`, file or workspace scope, one per
batch and attached to every parsed root). The source is registered under
`<name>.jssg.js` (non-alphanumeric characters of the name replaced), the
module name transform errors mention. Then:

1. in workspace mode (`indexesWorkspace`), every batch file is fed to the
   provider before the first transform (`BatchPlan::index`; QuickJS
   `jssgIndex`): the whole selected set, including files the selector will
   skip, so a matching transform can resolve definitions and references in
   them;
2. per chunk of 64 files, each path is contained in the root (`stat` +
   `realpath`, nearest existing ancestor) and the file parsed from the content
   the host supplied; a file the selector does not match (`selector_matches`)
   is recorded with no edits and no output and never reaches JavaScript;
3. each remaining file's transform runs with `write()` and `jssgTransform()`
   staged (never written) and contained to the target root, the equivalent of
   `stage_writes: true`; `options.matches` is undefined and the transform
   selects nodes itself;
4. the primary result, `jssgTransform` results, and staged `write()` results
   become `edits` (`BatchPlan::finish`, `classify_transform_result`);
   `Unmodified` and `Skipped` produce no edit.

Snapshot semantics: no transform sees another transform's edits, and the
index is not rebuilt between files. The workflow engine is incremental
instead: it writes each file before moving on and later files read it from
disk. This is the one behavioral difference on the JSSG path and is why
TypeScript treats two edits of one destination as a conflict rather than
chaining them.

The first transform error fails the whole batch; nothing is returned for
earlier files because nothing would be written anyway. Each artifact module is
evaluated once per run (cached by hash) and transforms run one at a time in
the realm, so module-level state persists across files and commands of one
run (unsupported). A batch of 64 files or more runs in chunks on the QuickJS
host's transform pool (`CODEMOD_JSSG_WORKERS` threads, else the core count
capped at 4), each thread with its own context and its own evaluation of the
artifact, so there that state is per worker.

The artifact is self-contained (esbuild inlined every helper), so there is no
module resolver, no tsconfig discovery, and no transpiler on this path: the
only imports left in an artifact are `codemod:*` modules and built-ins. A
legacy `getSelector` export in an artifact is ignored; Butterflow,
`codemod jssg run`, and `jssg list-applicable` keep executing `getSelector`
exactly as before through the unchanged `extract_selector_with_quickjs`.

### What `stage_writes` changes in the sandbox

`SgRoot.write()` on a root obtained from `definition()` or `references()`
writes straight to disk in the workflow engine. With
`JssgExecutionOptions::stage_writes` the same call validates the path against
the target directory and records a `FileChange` in the secondary results
instead. The QuickJS host always stages; the engine, CLI, and MCP
callers pass `false`, so their behavior is unchanged.

### The agent runner

`codemod-agent` (`crates/orchestration-agent`) executes one `agent`
operation over its own standard streams. It runs out of process, unlike every
other kind, because an agent's tools (the builtin `bash` and file tools, MCP
servers, the `claude` and `codex` CLIs) run in the process working directory
and inherit its environment; neither the control plane's directory nor its
environment is the right one to hand them.

- Location: `CODEMOD_AGENT_BIN`, else the newer of the monorepo's
  `target/{release,debug}/codemod-agent` (honouring `CARGO_TARGET_DIR`;
  `src/host/runtimes/agent-bin.ts`). For `quickjs` the Node launcher resolves
  it and passes it to the host in `CODEMOD_AGENT_BIN`. The path is checked
  only when an agent step runs; a missing runner is `failed` with
  `phase: "config"` and a message naming `cargo build -p orchestration-agent`.
- Launch (`executeAgent`, portable): `Host.spawn` in the target root, no
  arguments, the allowlisted environment of `agentLaunch` (see "Security
  model"), stderr ignored.
- stdin: one line of `OperationRequest` JSON, then, when there is a key,
  `{"LLM_API_KEY": "..."}`, with `CODEMOD_AGENT_SECRETS=stdin` in the
  environment. Requests over 64 MiB are refused.
- stdout: the `OperationCompletion` JSON and nothing else. Before any thread
  or child exists the runner moves stdout to a private close-on-exec
  descriptor and points fds 0 and 1 at `/dev/null`, so tool processes see
  neither the request nor the response stream.
- Exit codes: 0 completion written, 2 unexpected arguments, 3 unreadable or
  malformed request or another operation kind (an error completion is still
  written), 4 runtime failure (including stdout not writable).
- The launcher accepts only a completion for the same `commandId`.

Outcomes, as `executeAgent` reports them:

| status                                         | when                                                                                                                                                         | repository           |
| ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------- |
| `failed`, `phase: "config"`                    | refused before start: invalid settings, a credential passed to an external backend, an invalid `externalAgentTimeoutMs`, no runner binary                    | unchanged            |
| `cancelled`, `phase: "start"`                  | aborted before the runner was spawned                                                                                                                        | unchanged            |
| the runner's completion                        | `succeeded`, or `failed` with `phase: "config"` (`repositoryMayBeModified: false`) or `phase: "execute"` (`repositoryMayBeModified: true`) from inside it     | as reported          |
| `unknown`, `repositoryMayBeModified: true`     | started, then aborted, timed out (`timedOut: true`, external backends, default 30 minutes, `externalAgentTimeoutMs`), crashed, or no or a mismatched completion | may have been edited |

Cancellation kills the runner's whole process tree, including descendants
that left its process group (see "Security model").

## Security model

Every JSSG path is validated independently on both sides of the batch
boundary. Neither side trusts the other's check.

Rust (`crates/jssg-batch`):

- `targetRoot` must be absolute and canonicalize to a directory;
  `semanticAnalysis.root` must be a safe relative path (non-empty, not
  absolute on any platform including `C:\` and `\\server` forms, no `..`
  segment).
- `transform.name` must be non-empty and `transform.hash` a lowercase hex
  SHA-256 equal to the digest of `context.artifact.source`; a mismatch or a
  missing source fails the batch before any file is touched.
- `selector` must have a `rule` and at most `constraints` and `utils`; the
  rule is compiled by ast-grep before any file runs, so an invalid rule fails
  the batch.
- `files[].path` must be a safe relative path whose nearest existing ancestor
  (or the file itself) canonicalizes inside the target root, so a symlinked
  file or directory pointing outside is rejected before that file reaches a
  transform.
- `rename_to`, `jssgTransform` targets, and staged `write()` targets may be
  absolute or relative. `..` components are rejected, the same nearest
  existing ancestor check applies, and the result is normalized to a
  root-relative `/`-separated path.

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
  any file is selected.

What else a transform can reach:

- `quickjs` is the sandboxed runtime: transforms get the sandbox's built-in
  modules (`codemod:ast-grep`, `codemod:workflow`, `codemod:metrics`,
  `codemod:runtime`, the curated `fs` limited to the target root, LLRT's safe
  modules), no LLM module, no `child_process`, and no Node. The context has
  no global `fetch`; its only network primitive, `httpRequest`, reaches only
  the decision API root, resolved as `src/core/decision.ts` resolves it
  (`DECISION_BASE_URL`, else `TYPESAFE_BASE_URL` for `systemone`, else the
  provider's default, `https://api.typesafe.ai` or `https://api.openai.com`:
  same scheme, host, and port, a path under the root's path, redirects not
  followed), so workflow and transform code get no
  general network access.

A transform that writes through a filesystem module is outside the batch
result and is not tracked.

The agent runner (a trusted-local prototype, not a sandbox):

- Environment: the runner does not inherit the host environment. It gets the
  allowlist of `src/execution/agent-env.ts` (process basics, proxy and CA
  variables, `CLAUDE_CONFIG_DIR`/`CODEX_HOME`, and for `builtin` only the
  `LLM_*` settings) plus the runtime's explicit `env`. No host credentials
  (decision API keys, cloud, CI, registry tokens) unless passed explicitly.
  External backends get no `LLM_*` or provider variables, and an explicit
  credential-looking variable is refused for them as a `config` failure.
- The key: `LLM_API_KEY` never appears in the runner's launch environment
  (`ps eww`, `/proc/<pid>/environ`) or in the request; it travels on stdin
  after the request. The runner reads it before any thread starts and removes
  `LLM_API_KEY` and `CODEMOD_AGENT_SECRETS` from its own environment, so tool
  processes do not inherit them. Launched another way with `LLM_API_KEY` in
  its environment, removal only affects inheritance.
- The response channel: tool processes inherit `/dev/null` for fds 0 and 1,
  so they cannot read the request (or the key) from stdin or forge the
  completion on stdout.
- Tools are not confined: builtin file tools take any absolute path, and
  `bash` or `mcp_tool`, when named, run arbitrary commands as the user. The
  external CLIs confine themselves as far as their own flags enforce.
- Cancellation: `Host.spawn` starts the runner as a process-group leader;
  abort stops the group (SIGSTOP), takes one `ps` snapshot, kills descendants
  that left the group, then the group. Node's `killProcessTree` and the
  QuickJS host's `kill` do the same.

## Transaction semantics (TypeScript)

`executeJssgCommand` in `jssg.ts` runs one command, unchanged across
runtimes:

1. Look up the artifact by `operation.transform.hash` in the executor's
   store; a missing artifact fails in phase `artifact` before anything runs.
2. Resolve the target root beneath the working directory, select files
   (`files.ts`, engine walker semantics; see the README), and read them.
   Files that vanished or are not UTF-8 are skipped, as the engine does.
3. Hand the artifact source and the batch to the runtime's dispatch and wait.
   The dispatch never writes the repository and never throws.
4. Validate the outcomes, merge every edit into one write set, and reject
   what snapshot semantics cannot reconcile: two edits to one destination, a
   source renamed twice, a write to a path another edit renames away, and a
   rename onto a file that exists unless that file is itself renamed away.
5. If the signal has not fired, write every destination (creating
   directories), then remove rename sources.

Nothing touches the repository before step 5. Failure classification:

| status      | when                                                                     | repository |
| ----------- | ------------------------------------------------------------------------ | ---------- |
| `failed`    | artifact, select, transform (batch), invalid result, or a conflict       | unchanged  |
| `cancelled` | the signal fired before commit (checked between files and before commit) | unchanged  |
| `unknown`   | commit stopped part-way                                                  | partial    |
| `succeeded` | every write and deletion applied                                         | committed  |

`error.details.phase` is `artifact`, `select`, `transform`, `stage`, or
`commit`; a commit failure also carries `applied`, `failed`, and `remaining`.
Per-file writes are ordinary `Host.fs.writeText` calls; there is no
cross-file transaction on ordinary filesystems.

## Why this shape

- Rust keeps what only Rust can do: ast-grep, the semantic providers, and the
  batch rules, in one crate (`jssg-batch`) the runtime calls, so nothing
  re-implements them in TypeScript.
- Everything that is policy (which files, in what order, what counts as a
  conflict, when to write, how to classify a failure) and everything that is
  build (extraction, bundling, identity) is TypeScript, where the authoring
  model lives and iterates quickly. The batch side cannot write repository
  files even if asked, and cannot run a transform whose source does not match
  the identity the workflow recorded.
- One JSON request and one JSON completion per operation means the same
  operation can be executed in-process, scripted by the test harness, or sent
  to a process (`codemod-agent`) without changing the control plane.

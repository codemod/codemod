/**
 * Versioned JSON wire protocol between the TypeScript runtime and any
 * OperationExecutor, including the Rust execution bridge
 * (`crates/execution-bridge`). Keep this file JSON-only: no classes, no
 * functions on the wire. The Rust serde structs mirror these shapes exactly;
 * `fixtures/protocol/*.json` are the shared conformance fixtures.
 */
import {
  isAssessmentEntry,
  isAssessmentQuestions,
  type AssessmentQuestions,
  type AssessmentState,
} from "./assessment.ts";
import type { Json } from "./json.ts";
import { isSafeRelativePath } from "./paths.ts";

export const PROTOCOL_VERSION = 8 as const;

export type CompletionStatus = "succeeded" | "failed" | "cancelled" | "unknown";

export interface ShellOperation {
  kind: "shell";
  command: string;
  env?: Record<string, string>;
}

/**
 * Repository area one JSSG invocation applies to. `root` is a directory
 * relative to the executor's working directory; `include` and `exclude` are
 * glob patterns relative to `root`. The effective file set is the
 * intersection of this target with the JSSG definition's own applicability.
 * Author input is validated and normalized by `authoring/target.ts`; on the wire this is
 * plain data and part of the command content that replay compares.
 */
export interface Target {
  root?: string;
  include?: string[];
  exclude?: string[];
}

/**
 * `"file"`, `"workspace"`, or the object form. `root` is a safe relative path
 * beneath the invocation's target root and is only valid with `workspace`.
 */
export type SemanticAnalysis = "file" | "workspace" | { mode: "file" | "workspace"; root?: string };

/**
 * Build-time identity of one transform artifact: the definition's `name` and
 * the lowercase hex SHA-256 of the bundled source (`bundle/build.ts`). It carries no
 * path, so the command identity recorded in history is the same on every
 * checkout; the source itself travels in `RequestContext.artifact`.
 */
export interface ArtifactRef {
  name: string;
  hash: string;
}

/**
 * Static eligibility prefilter: ast-grep rule data (`rule`, optional
 * `constraints` and `utils`) evaluated natively by the executor before any
 * transform sandbox starts. Files without a match are skipped. `id` and
 * `language` are supplied by the executor and may not appear here.
 */
export interface Selector {
  rule: { [key: string]: Json };
  constraints?: { [key: string]: Json };
  utils?: { [key: string]: Json };
}

/**
 * JSSG codemod invocation. Only this operation carries a `target`: a JSSG
 * adapter is the only executor that can enumerate and enforce a file set.
 * Definition fields are intrinsic applicability. `target` can only narrow them.
 */
export interface JssgOperation {
  kind: "jssg";
  transform: ArtifactRef;
  language: string;
  include?: string[];
  exclude?: string[];
  semanticAnalysis?: SemanticAnalysis;
  selector?: Selector;
  target?: Target;
  input?: Json;
}

/**
 * The built-in agent's tools, by their `codemod-ai` names (the YAML `ai` step
 * `tools` list). `bash` runs arbitrary commands and `mcp_tool` starts
 * arbitrary server processes; neither is confined to the repository.
 */
export const BUILTIN_AGENT_TOOLS = [
  "bash",
  "str_replace_based_edit_tool",
  "json_edit_tool",
  "glob",
  "sequentialthinking",
  "task_done",
  "ckg_tool",
  "mcp_tool",
] as const;

export type BuiltinAgentTool = (typeof BUILTIN_AGENT_TOOLS)[number];

/**
 * What a builtin agent gets when it names no tools: file viewing and editing,
 * globbing, planning, and completion. No shell, no MCP servers, no code
 * knowledge graph database written into the repository.
 */
export const DEFAULT_BUILTIN_AGENT_TOOLS: readonly BuiltinAgentTool[] = [
  "str_replace_based_edit_tool",
  "json_edit_tool",
  "glob",
  "sequentialthinking",
  "task_done",
];

/**
 * Claude Code built-in tools a `claude-code` agent may be given. The bridge
 * passes the list as both `--tools` (what exists) and `--allowedTools` (what
 * runs without a prompt); nothing else is available, and anything that would
 * still ask for permission is denied. `Bash` runs arbitrary commands.
 */
export const CLAUDE_CODE_TOOLS = ["Read", "Edit", "Write", "Glob", "Grep", "Bash"] as const;

export type ClaudeCodeTool = (typeof CLAUDE_CODE_TOOLS)[number];

/** Read and edit files in the working directory; no shell. */
export const DEFAULT_CLAUDE_CODE_TOOLS: readonly ClaudeCodeTool[] = [
  "Read",
  "Edit",
  "Write",
  "Glob",
  "Grep",
];

/**
 * Codex `exec --sandbox` modes a `codex` agent may use. `danger-full-access`
 * is deliberately not representable.
 */
export const CODEX_SANDBOXES = ["read-only", "workspace-write"] as const;

export type CodexSandbox = (typeof CODEX_SANDBOXES)[number];

export const DEFAULT_CODEX_SANDBOX: CodexSandbox = "workspace-write";

/**
 * Which agent loop runs the task, with only the settings that backend
 * enforces:
 *
 * - `builtin`: `codemod-ai` (Rig) with exactly `tools`; `maxSteps` bounds its
 *   turns (its own default is 30). Uses `LLM_API_KEY`.
 * - `claude-code`: the installed, logged-in `claude` CLI with exactly `tools`.
 * - `codex`: the installed, logged-in `codex` CLI in `sandbox`.
 *
 * External backends own their agent loop, use the CLI's own login and quota,
 * and never see `LLM_API_KEY`.
 */
export type AgentBackend =
  | { kind: "builtin"; tools: BuiltinAgentTool[]; maxSteps?: number }
  | { kind: "claude-code"; tools: ClaudeCodeTool[] }
  | { kind: "codex"; sandbox: CodexSandbox };

export const AGENT_BACKENDS = ["builtin", "claude-code", "codex"] as const;

export type AgentBackendKind = AgentBackend["kind"];

/**
 * An agent task: a prompt plus optional input, run by the bridge in the
 * executor's working directory through `backend`. It may change files. A
 * succeeded completion's output is `{ text }`, the agent's final response;
 * `responseFormat: "json"` makes the bridge ask the agent for a JSON reply;
 * the bridge does not check the reply, TypeScript decoding
 * (`parseAgentJson` plus the step's output schema) enforces it.
 */
export interface AgentOperation {
  kind: "agent";
  prompt: string;
  input?: Json;
  backend: AgentBackend;
  responseFormat?: "json";
}

/**
 * A read-only System One assessment (`core/assessment.ts`): explicit `state`
 * and named typed questions, no repository access, no tools. `model` pins a
 * model; without it the executor's default answers (`jev-latest` unless
 * configured). A succeeded completion's output is an `AssessmentResult`.
 */
export interface AssessmentOperation {
  kind: "assessment";
  state: AssessmentState;
  questions: AssessmentQuestions;
  model?: string;
}

export type Operation = ShellOperation | JssgOperation | AgentOperation | AssessmentOperation;

/** One selected file, already read by the host. `path` is target-root-relative. */
export interface BatchFile {
  path: string;
  content: string;
}

/**
 * Executor-side context. It is attached by the host that runs an executor,
 * never by workflow code, and it is not part of the command record that
 * history stores, so it may carry machine-specific absolute paths, file
 * contents, and the transform source.
 */
export interface RequestContext {
  /** Absolute directory every JSSG file path is relative to. */
  targetRoot?: string;
  /** The JSSG batch: selected files in transform order. */
  files?: BatchFile[];
  /** The bundled transform whose hash `JssgOperation.transform` names. */
  artifact?: { source: string };
}

export interface OperationRequest {
  protocolVersion: typeof PROTOCOL_VERSION;
  commandId: string;
  operation: Operation;
  context?: RequestContext;
}

/**
 * One write the sandbox asked for: `content` belongs at `renameTo` when set
 * (and `path` goes away), otherwise at `path`. Both are target-root-relative.
 */
export interface Edit {
  path: string;
  content: string;
  renameTo?: string;
}

/**
 * What one batch file's transform produced: its own edit if modified,
 * `jssgTransform` and `write()` edits, and the `StructuredCodemod` output.
 * A file the selector skipped has no edits and no output. The bridge returns
 * `{ files: FileOutcome[] }` in batch order.
 */
export interface FileOutcome {
  path: string;
  edits: Edit[];
  output?: Json;
}

export interface CompletionError {
  message: string;
  exitCode?: number;
  output?: string;
  /**
   * Structured failure detail. JSSG commands report the phase that failed
   * (`artifact`, `select`, `transform`, `stage`, `commit`) and for commit
   * failures which files were already applied.
   */
  details?: Json;
}

export type OperationCompletion =
  | {
      protocolVersion: typeof PROTOCOL_VERSION;
      commandId: string;
      status: "succeeded";
      /**
       * Shell `{ stdout }`; jssg the per-file structured outputs in file order;
       * agent `{ text }`; assessment `{ model, answers, usage }`.
       */
      output: Json;
      error?: never;
    }
  | {
      protocolVersion: typeof PROTOCOL_VERSION;
      commandId: string;
      status: Exclude<CompletionStatus, "succeeded">;
      output?: never;
      error: CompletionError;
    };

const STATUSES: readonly CompletionStatus[] = ["succeeded", "failed", "cancelled", "unknown"];

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isStringMap(value: unknown): value is Record<string, string> {
  return isRecord(value) && Object.values(value).every((v) => typeof v === "string");
}

export function isJson(value: unknown): value is Json {
  if (value === null || typeof value === "string" || typeof value === "boolean") return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.every(isJson);
  return isRecord(value) && Object.values(value).every(isJson);
}

function isStringList(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((v) => typeof v === "string");
}

function isNonEmptyStringList(value: unknown): value is string[] {
  return isStringList(value) && value.length > 0 && value.every((item) => item.trim() !== "");
}

function isNonBlank(value: unknown): value is string {
  return typeof value === "string" && value.trim() !== "";
}

export function hasOnlyKeys(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  return Object.keys(value).every((key) => allowed.includes(key));
}

/**
 * Field sets per operation kind. Validation is strict: a field from another
 * variant, most importantly a `target` on `shell`, `agent`, or `assessment`, makes the operation
 * invalid rather than being ignored. Mirrors `deny_unknown_fields` in the Rust bridge.
 */
const OPERATION_FIELDS = {
  shell: ["kind", "command", "env"],
  jssg: [
    "kind",
    "transform",
    "language",
    "include",
    "exclude",
    "semanticAnalysis",
    "selector",
    "target",
    "input",
  ],
  agent: ["kind", "prompt", "input", "backend", "responseFormat"],
  assessment: ["kind", "state", "questions", "model"],
} as const satisfies Record<Operation["kind"], readonly string[]>;

/**
 * Wire shape plus the path rule the bridge also enforces (`root` is a safe
 * relative path). Normalization and non-empty lists live in `authoring/target.ts`.
 */
export function isTarget(value: unknown): value is Target {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["root", "include", "exclude"]) &&
    (value.root === undefined ||
      (typeof value.root === "string" && isSafeRelativePath(value.root))) &&
    (value.include === undefined || isStringList(value.include)) &&
    (value.exclude === undefined || isStringList(value.exclude))
  );
}

export function isSemanticAnalysis(value: unknown): value is SemanticAnalysis {
  if (value === "file" || value === "workspace") return true;
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["mode", "root"]) &&
    (value.mode === "file" || value.mode === "workspace") &&
    (value.root === undefined ||
      (value.mode === "workspace" &&
        typeof value.root === "string" &&
        isSafeRelativePath(value.root)))
  );
}

/** A non-empty name and a lowercase hex SHA-256; the bridge checks the same. */
export function isArtifactRef(value: unknown): value is ArtifactRef {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["name", "hash"]) &&
    isNonBlank(value.name) &&
    typeof value.hash === "string" &&
    /^[0-9a-f]{64}$/u.test(value.hash)
  );
}

function isJsonRecord(value: unknown): value is { [key: string]: Json } {
  return isRecord(value) && isJson(value);
}

export function isSelector(value: unknown): value is Selector {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["rule", "constraints", "utils"]) &&
    isJsonRecord(value.rule) &&
    Object.keys(value.rule).length > 0 &&
    (value.constraints === undefined || isJsonRecord(value.constraints)) &&
    (value.utils === undefined || isJsonRecord(value.utils))
  );
}

function isBatchFile(value: unknown): value is BatchFile {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["path", "content"]) &&
    typeof value.path === "string" &&
    isSafeRelativePath(value.path) &&
    typeof value.content === "string"
  );
}

function isRequestContext(value: unknown): value is RequestContext {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["targetRoot", "files", "artifact"]) &&
    (value.targetRoot === undefined || isNonBlank(value.targetRoot)) &&
    (value.files === undefined || (Array.isArray(value.files) && value.files.every(isBatchFile))) &&
    (value.artifact === undefined ||
      (isRecord(value.artifact) &&
        hasOnlyKeys(value.artifact, ["source"]) &&
        typeof value.artifact.source === "string"))
  );
}

function isEdit(value: unknown): value is Edit {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["path", "content", "renameTo"]) &&
    typeof value.path === "string" &&
    isSafeRelativePath(value.path) &&
    typeof value.content === "string" &&
    (value.renameTo === undefined ||
      (typeof value.renameTo === "string" && isSafeRelativePath(value.renameTo)))
  );
}

/** Every path the bridge returns must be a safe relative path. */
export function isFileOutcomes(value: unknown): value is FileOutcome[] {
  return (
    Array.isArray(value) &&
    value.every(
      (outcome) =>
        isRecord(outcome) &&
        hasOnlyKeys(outcome, ["path", "edits", "output"]) &&
        typeof outcome.path === "string" &&
        isSafeRelativePath(outcome.path) &&
        Array.isArray(outcome.edits) &&
        outcome.edits.every(isEdit) &&
        (outcome.output === undefined || isJson(outcome.output)),
    )
  );
}

function isCompletionError(value: unknown): value is CompletionError {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["message", "exitCode", "output", "details"]) &&
    typeof value.message === "string" &&
    (value.exitCode === undefined || Number.isInteger(value.exitCode)) &&
    (value.output === undefined || typeof value.output === "string") &&
    (value.details === undefined || isJson(value.details))
  );
}

/** Names from `known`, each at most once. An empty list is a tool-less agent. */
export function isToolList<T extends string>(value: unknown, known: readonly T[]): value is T[] {
  return (
    Array.isArray(value) &&
    value.every((tool) => (known as readonly unknown[]).includes(tool)) &&
    new Set(value).size === value.length
  );
}

/**
 * Why a backend is malformed or asks for a setting its backend cannot
 * enforce, or `undefined` when it is valid.
 */
export function agentBackendProblem(value: unknown): string | undefined {
  if (!isRecord(value)) return "backend must be an object";
  const only = (allowed: readonly string[]) =>
    Object.keys(value).find((key) => !allowed.includes(key));
  const list = (known: readonly string[]) => known.map((name) => `'${name}'`).join(", ");
  switch (value.kind) {
    case "builtin": {
      const extra = only(["kind", "tools", "maxSteps"]);
      if (extra !== undefined) return `backend 'builtin' does not support '${extra}'`;
      if (!isToolList(value.tools, BUILTIN_AGENT_TOOLS)) {
        return `builtin tools must be distinct names from ${list(BUILTIN_AGENT_TOOLS)}`;
      }
      if (value.maxSteps !== undefined && !isMaxSteps(value.maxSteps)) {
        return "builtin maxSteps must be a positive integer";
      }
      return undefined;
    }
    case "claude-code": {
      const extra = only(["kind", "tools"]);
      if (extra !== undefined) return `backend 'claude-code' does not support '${extra}'`;
      return isToolList(value.tools, CLAUDE_CODE_TOOLS)
        ? undefined
        : `claude-code tools must be distinct names from ${list(CLAUDE_CODE_TOOLS)}`;
    }
    case "codex": {
      const extra = only(["kind", "sandbox"]);
      if (extra !== undefined) return `backend 'codex' does not support '${extra}'`;
      return (CODEX_SANDBOXES as readonly unknown[]).includes(value.sandbox)
        ? undefined
        : `codex sandbox must be one of ${list(CODEX_SANDBOXES)}`;
    }
    default:
      return `backend kind must be one of ${list(AGENT_BACKENDS)}`;
  }
}

export function isAgentBackend(value: unknown): value is AgentBackend {
  return agentBackendProblem(value) === undefined;
}

export function isMaxSteps(value: unknown): value is number {
  return typeof value === "number" && Number.isInteger(value) && value >= 1;
}

export function isOperation(value: unknown): value is Operation {
  if (!isRecord(value)) return false;
  switch (value.kind) {
    case "shell":
      return (
        hasOnlyKeys(value, OPERATION_FIELDS.shell) &&
        typeof value.command === "string" &&
        (value.env === undefined || isStringMap(value.env))
      );
    case "jssg":
      return (
        hasOnlyKeys(value, OPERATION_FIELDS.jssg) &&
        isArtifactRef(value.transform) &&
        isNonBlank(value.language) &&
        (value.include === undefined || isNonEmptyStringList(value.include)) &&
        (value.exclude === undefined || isNonEmptyStringList(value.exclude)) &&
        (value.semanticAnalysis === undefined || isSemanticAnalysis(value.semanticAnalysis)) &&
        (value.selector === undefined || isSelector(value.selector)) &&
        (value.target === undefined || isTarget(value.target)) &&
        (value.input === undefined || isJson(value.input))
      );
    case "agent":
      return (
        hasOnlyKeys(value, OPERATION_FIELDS.agent) &&
        typeof value.prompt === "string" &&
        (value.input === undefined || isJson(value.input)) &&
        isAgentBackend(value.backend) &&
        (value.responseFormat === undefined || value.responseFormat === "json")
      );
    case "assessment":
      return (
        hasOnlyKeys(value, OPERATION_FIELDS.assessment) &&
        isAssessmentEntry(value.state) &&
        isAssessmentQuestions(value.questions) &&
        (value.model === undefined || isNonBlank(value.model))
      );
    default:
      return false;
  }
}

export function isOperationRequest(value: unknown): value is OperationRequest {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["protocolVersion", "commandId", "operation", "context"]) &&
    value.protocolVersion === PROTOCOL_VERSION &&
    typeof value.commandId === "string" &&
    isOperation(value.operation) &&
    (value.context === undefined || isRequestContext(value.context))
  );
}

export function isOperationCompletion(value: unknown): value is OperationCompletion {
  if (!isRecord(value)) return false;
  if (!hasOnlyKeys(value, ["protocolVersion", "commandId", "status", "output", "error"])) {
    return false;
  }
  if (value.protocolVersion !== PROTOCOL_VERSION) return false;
  if (typeof value.commandId !== "string") return false;
  if (!STATUSES.includes(value.status as CompletionStatus)) return false;
  if (value.status === "succeeded") return isJson(value.output) && value.error === undefined;
  return value.output === undefined && isCompletionError(value.error);
}

/** Parse a completion produced by an external executor, e.g. the Rust bridge. */
export function parseCompletion(text: string): OperationCompletion {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (error) {
    throw new Error(`executor returned invalid JSON: ${(error as Error).message}`);
  }
  if (!isOperationCompletion(value)) {
    throw new Error(`executor returned an invalid completion: ${text}`);
  }
  return value;
}

/**
 * Versioned JSON wire protocol between the TypeScript runtime and any
 * OperationExecutor, including the Rust execution bridge
 * (`crates/execution-bridge`). Keep this file JSON-only: no classes, no
 * functions on the wire. The Rust serde structs mirror these shapes exactly;
 * `fixtures/protocol/*.json` are the shared conformance fixtures. The JSSG
 * worker messages live in `worker-protocol.ts`.
 */
import type { Json } from "./json.ts";
import { isSafeRelativePath } from "./paths.ts";

export const PROTOCOL_VERSION = 3 as const;

export type CompletionStatus = "succeeded" | "failed" | "cancelled" | "unknown";

export interface ExecOperation {
  kind: "exec";
  command: string;
  env?: Record<string, string>;
}

/**
 * Repository area one JSSG invocation applies to. `root` is a directory
 * relative to the executor's working directory; `include` and `exclude` are
 * glob patterns relative to `root`. The effective file set is the
 * intersection of this target with the JSSG definition's own applicability.
 * Author input is validated and normalized by `target.ts`; on the wire this is
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
 * JSSG codemod invocation. Only this operation carries a `target`: a JSSG
 * adapter is the only executor that can enumerate and enforce a file set.
 * Definition fields are intrinsic applicability. `target` can only narrow them.
 */
export interface JssgOperation {
  kind: "jssg";
  /**
   * Safe relative path to the transform, resolved by the executor against
   * its script root. Never absolute, so the command identity recorded in
   * history is the same on every checkout.
   */
  script: string;
  language: string;
  include?: string[];
  exclude?: string[];
  semanticAnalysis?: SemanticAnalysis;
  target?: Target;
  input?: Json;
}

/** Placeholder for the future AI OperationExecutor adapter. Not executed by the bridge. */
export interface AiOperation {
  kind: "ai";
  prompt: string;
  input?: Json;
}

export type Operation = ExecOperation | JssgOperation | AiOperation;

/**
 * Executor-side context. It is attached by the host that runs an executor,
 * never by workflow code, and it is not part of the command record that
 * history stores, so it may carry machine-specific absolute paths.
 */
export interface RequestContext {
  /** Directory that relative JSSG `script` paths are resolved against. */
  scriptRoot?: string;
}

export interface OperationRequest {
  protocolVersion: typeof PROTOCOL_VERSION;
  commandId: string;
  operation: Operation;
  context?: RequestContext;
}

export interface CompletionError {
  message: string;
  exitCode?: number;
  output?: string;
  /**
   * Structured failure detail. JSSG commands report the phase that failed
   * (`open`, `select`, `index`, `transform`, `stage`, `commit`), the file
   * involved, and for commit failures which files were already applied.
   */
  details?: Json;
}

export type OperationCompletion =
  | {
      protocolVersion: typeof PROTOCOL_VERSION;
      commandId: string;
      status: "succeeded";
      /** For exec this is `{ stdout: string }`; for jssg the per-file outputs in file order. */
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

export function isStringList(value: unknown): value is string[] {
  return Array.isArray(value) && value.every((v) => typeof v === "string");
}

function isNonEmptyStringList(value: unknown): value is string[] {
  return isStringList(value) && value.length > 0 && value.every((item) => item.trim() !== "");
}

export function hasOnlyKeys(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  return Object.keys(value).every((key) => allowed.includes(key));
}

const TARGET_FIELDS = ["root", "include", "exclude"] as const;

/**
 * Field sets per operation kind. Validation is strict: a field from another
 * variant, most importantly a `target` on `exec` or `ai`, makes the operation
 * invalid rather than being ignored. Mirrors `deny_unknown_fields` in the Rust bridge.
 */
const OPERATION_FIELDS = {
  exec: ["kind", "command", "env"],
  jssg: ["kind", "script", "language", "include", "exclude", "semanticAnalysis", "target", "input"],
  ai: ["kind", "prompt", "input"],
} as const satisfies Record<Operation["kind"], readonly string[]>;

/**
 * Wire shape plus the path rule the bridge also enforces (`root` is a safe
 * relative path). Normalization and non-empty lists live in `target.ts`.
 */
export function isTarget(value: unknown): value is Target {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, TARGET_FIELDS) &&
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

function isRequestContext(value: unknown): value is RequestContext {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["scriptRoot"]) &&
    (value.scriptRoot === undefined ||
      (typeof value.scriptRoot === "string" && value.scriptRoot.trim() !== ""))
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

export function isOperation(value: unknown): value is Operation {
  if (!isRecord(value)) return false;
  switch (value.kind) {
    case "exec":
      return (
        hasOnlyKeys(value, OPERATION_FIELDS.exec) &&
        typeof value.command === "string" &&
        (value.env === undefined || isStringMap(value.env))
      );
    case "jssg":
      return (
        hasOnlyKeys(value, OPERATION_FIELDS.jssg) &&
        typeof value.script === "string" &&
        isSafeRelativePath(value.script) &&
        typeof value.language === "string" &&
        value.language.trim() !== "" &&
        (value.include === undefined || isNonEmptyStringList(value.include)) &&
        (value.exclude === undefined || isNonEmptyStringList(value.exclude)) &&
        (value.semanticAnalysis === undefined || isSemanticAnalysis(value.semanticAnalysis)) &&
        (value.target === undefined || isTarget(value.target)) &&
        (value.input === undefined || isJson(value.input))
      );
    case "ai":
      return (
        hasOnlyKeys(value, OPERATION_FIELDS.ai) &&
        typeof value.prompt === "string" &&
        (value.input === undefined || isJson(value.input))
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

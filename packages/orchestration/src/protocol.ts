/**
 * Versioned JSON wire protocol between the TypeScript runtime and any
 * OperationExecutor, including the Rust execution bridge
 * (`crates/execution-bridge`). Keep this file JSON-only: no classes, no
 * functions on the wire. The Rust serde structs mirror these shapes exactly;
 * `fixtures/protocol/*.json` are the shared conformance fixtures.
 */
import type { Json } from "./json.ts";

export const PROTOCOL_VERSION = 1 as const;

export type CompletionStatus = "succeeded" | "failed" | "cancelled" | "unknown";

export interface ExecOperation {
  kind: "exec";
  command: string;
  env?: Record<string, string>;
}

/** Placeholder for the future JSSG OperationExecutor adapter. Not executed by the bridge. */
export interface JssgOperation {
  kind: "jssg";
  package: string;
  input?: Json;
}

/** Placeholder for the future AI OperationExecutor adapter. Not executed by the bridge. */
export interface AiOperation {
  kind: "ai";
  prompt: string;
  input?: Json;
}

export type Operation = ExecOperation | JssgOperation | AiOperation;

export interface OperationRequest {
  protocolVersion: typeof PROTOCOL_VERSION;
  commandId: string;
  operation: Operation;
}

export interface CompletionError {
  message: string;
  exitCode?: number;
  output?: string;
}

export interface OperationCompletion {
  protocolVersion: typeof PROTOCOL_VERSION;
  commandId: string;
  status: CompletionStatus;
  /** Present when status is `succeeded`. For exec this is `{ stdout: string }`. */
  output?: Json;
  /** Present when status is not `succeeded`. */
  error?: CompletionError;
}

const STATUSES: readonly CompletionStatus[] = ["succeeded", "failed", "cancelled", "unknown"];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isStringMap(value: unknown): value is Record<string, string> {
  return isRecord(value) && Object.values(value).every((v) => typeof v === "string");
}

export function isOperation(value: unknown): value is Operation {
  if (!isRecord(value)) return false;
  switch (value.kind) {
    case "exec":
      return (
        typeof value.command === "string" && (value.env === undefined || isStringMap(value.env))
      );
    case "jssg":
      return typeof value.package === "string";
    case "ai":
      return typeof value.prompt === "string";
    default:
      return false;
  }
}

export function isOperationRequest(value: unknown): value is OperationRequest {
  return (
    isRecord(value) &&
    value.protocolVersion === PROTOCOL_VERSION &&
    typeof value.commandId === "string" &&
    isOperation(value.operation)
  );
}

export function isOperationCompletion(value: unknown): value is OperationCompletion {
  if (!isRecord(value)) return false;
  if (value.protocolVersion !== PROTOCOL_VERSION) return false;
  if (typeof value.commandId !== "string") return false;
  if (!STATUSES.includes(value.status as CompletionStatus)) return false;
  if (value.error !== undefined) {
    if (!isRecord(value.error) || typeof value.error.message !== "string") return false;
  }
  return true;
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

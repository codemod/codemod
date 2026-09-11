/**
 * JSONL protocol between the TypeScript JSSG orchestrator and one persistent
 * Rust worker (`butterflow-execution-bridge --jssg-worker`). One JSON object
 * per line in each direction, strictly request/response in order. The Rust
 * side is `crates/execution-bridge/src/worker.rs`; both sides reject unknown
 * fields. Keep this file JSON-only.
 *
 * The worker holds one loaded script, its selector, the language, the
 * invocation input, and an optional semantic provider. It never lists the
 * repository and never writes repository files: the host sends file contents
 * and receives edits as data.
 */
import type { Json } from "./json.ts";
import { isSafeRelativePath } from "./paths.ts";
import {
  PROTOCOL_VERSION,
  hasOnlyKeys,
  isJson,
  isRecord,
  isSemanticAnalysis,
  isStringList,
  type SemanticAnalysis,
} from "./protocol.ts";

export interface OpenRequest {
  type: "open";
  protocolVersion: typeof PROTOCOL_VERSION;
  /** Safe relative script path, resolved beneath `scriptRoot`. */
  script: string;
  /** Absolute; executor context, never command identity. */
  scriptRoot: string;
  language: string;
  /** Absolute; every path in later messages is relative to it. */
  targetRoot: string;
  semanticAnalysis?: SemanticAnalysis;
  input?: Json;
}

export interface IndexRequest {
  type: "index";
  path: string;
  content: string;
}

export interface TransformRequest {
  type: "transform";
  path: string;
  content: string;
}

export interface CloseRequest {
  type: "close";
}

export type WorkerRequest = OpenRequest | IndexRequest | TransformRequest | CloseRequest;

/** One file's result. `renameTo` is a target-root-relative path. */
export type FileResult =
  | { kind: "modified"; content: string; renameTo?: string }
  | { kind: "unmodified" }
  | { kind: "skipped" };

export interface SecondaryResult {
  /** Target-root-relative path of the file a `jssgTransform` or `write()` edited. */
  path: string;
  result: FileResult;
}

export interface TransformResult {
  primary: FileResult;
  secondary: SecondaryResult[];
  /** JSON from a `StructuredCodemod`; absent for legacy `string | null` returns. */
  output?: Json;
}

export type WorkerResponse =
  | {
      type: "opened";
      protocolVersion: typeof PROTOCOL_VERSION;
      /** The language's file extensions (`.ts`, ...), the definition's default include set. */
      extensions: string[];
      semanticMode: "file" | "workspace" | null;
    }
  | { type: "indexed" }
  | { type: "transformed"; result: TransformResult }
  | { type: "closed" }
  | {
      type: "error";
      message: string;
      /** `true`: the worker exits after this line. `false`: the session stays usable. */
      fatal: boolean;
    };

function isFileResult(value: unknown): value is FileResult {
  if (!isRecord(value)) return false;
  switch (value.kind) {
    case "modified":
      return (
        hasOnlyKeys(value, ["kind", "content", "renameTo"]) &&
        typeof value.content === "string" &&
        (value.renameTo === undefined ||
          (typeof value.renameTo === "string" && isSafeRelativePath(value.renameTo)))
      );
    case "unmodified":
    case "skipped":
      return hasOnlyKeys(value, ["kind"]);
    default:
      return false;
  }
}

function isSecondaryResult(value: unknown): value is SecondaryResult {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["path", "result"]) &&
    typeof value.path === "string" &&
    isSafeRelativePath(value.path) &&
    isFileResult(value.result)
  );
}

export function isTransformResult(value: unknown): value is TransformResult {
  return (
    isRecord(value) &&
    hasOnlyKeys(value, ["primary", "secondary", "output"]) &&
    isFileResult(value.primary) &&
    Array.isArray(value.secondary) &&
    value.secondary.every(isSecondaryResult) &&
    (value.output === undefined || isJson(value.output))
  );
}

export function isWorkerRequest(value: unknown): value is WorkerRequest {
  if (!isRecord(value)) return false;
  switch (value.type) {
    case "open":
      return (
        hasOnlyKeys(value, [
          "type",
          "protocolVersion",
          "script",
          "scriptRoot",
          "language",
          "targetRoot",
          "semanticAnalysis",
          "input",
        ]) &&
        value.protocolVersion === PROTOCOL_VERSION &&
        typeof value.script === "string" &&
        isSafeRelativePath(value.script) &&
        typeof value.scriptRoot === "string" &&
        value.scriptRoot.trim() !== "" &&
        typeof value.language === "string" &&
        value.language.trim() !== "" &&
        typeof value.targetRoot === "string" &&
        value.targetRoot.trim() !== "" &&
        (value.semanticAnalysis === undefined || isSemanticAnalysis(value.semanticAnalysis)) &&
        (value.input === undefined || isJson(value.input))
      );
    case "index":
    case "transform":
      return (
        hasOnlyKeys(value, ["type", "path", "content"]) &&
        typeof value.path === "string" &&
        isSafeRelativePath(value.path) &&
        typeof value.content === "string"
      );
    case "close":
      return hasOnlyKeys(value, ["type"]);
    default:
      return false;
  }
}

export function isWorkerResponse(value: unknown): value is WorkerResponse {
  if (!isRecord(value)) return false;
  switch (value.type) {
    case "opened":
      return (
        hasOnlyKeys(value, ["type", "protocolVersion", "extensions", "semanticMode"]) &&
        value.protocolVersion === PROTOCOL_VERSION &&
        isStringList(value.extensions) &&
        (value.semanticMode === null ||
          value.semanticMode === "file" ||
          value.semanticMode === "workspace")
      );
    case "indexed":
    case "closed":
      return hasOnlyKeys(value, ["type"]);
    case "transformed":
      return hasOnlyKeys(value, ["type", "result"]) && isTransformResult(value.result);
    case "error":
      return (
        hasOnlyKeys(value, ["type", "message", "fatal"]) &&
        typeof value.message === "string" &&
        typeof value.fatal === "boolean"
      );
    default:
      return false;
  }
}

/** Parse one response line from the worker. */
export function parseWorkerResponse(line: string): WorkerResponse {
  let value: unknown;
  try {
    value = JSON.parse(line);
  } catch (error) {
    throw new Error(`worker returned invalid JSON: ${(error as Error).message}`);
  }
  if (!isWorkerResponse(value)) throw new Error(`worker returned an invalid message: ${line}`);
  return value;
}

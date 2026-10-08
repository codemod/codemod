/**
 * Assessment adapter: one `assessment` operation becomes one
 * `TypeSafeClient.systemOne()` call through the official TypeSafe SDK
 * (`@typesafe-ai/sdk`), in the trusted host process.
 *
 * The SDK owns authentication, the `TYPESAFE_*` fallbacks, request encoding,
 * transport, response decoding, option validation, per-attempt timeouts, and
 * retries (its `RetryPolicy`); nothing here retries. The run's `AbortSignal`
 * is handed to the SDK, which cancels the in-flight attempt and any backoff.
 *
 * This module owns the workflow contract: SDK logging is off (non-CLI
 * packages must not write to the terminal), the few settings the SDK does
 * not bound are checked (timer overflow, a retry ceiling, an http(s) base
 * URL), SDK error classes map to stable completion phases, and the decoded
 * body is normalized and validated against the questions because the SDK
 * returns it unchecked. A failed assessment cannot have changed the
 * repository, so errors are `failed` and aborts `cancelled`, never `unknown`.
 */
import {
  APIConnectionError,
  APIError,
  APITimeoutError,
  APIUserAbortError,
  RateLimitError,
  TypeSafeClient,
  type Questions,
  type RequestOptions,
  type RetryPolicy,
  type SystemOneRequest,
  type TypeSafeClientConfig,
} from "@typesafe-ai/sdk";
import { readFileSync, realpathSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  ANSWER_FIELDS,
  assessmentResultProblem,
  getAssessmentAsk,
  questionsProblem,
  type AssessmentAskFn,
  type AssessmentQuestions,
} from "../core/assessment.ts";
import type { Json } from "../core/json.ts";
import {
  PROTOCOL_VERSION,
  isRecord,
  type AssessmentOperation,
  type OperationCompletion,
} from "../core/protocol.ts";
import { selectFiles } from "./files.ts";

/** The part of the SDK client used here; `TypeSafeClient` satisfies it. */
export interface AssessmentClient {
  /** API root after the SDK applied `baseURL`, `TYPESAFE_BASE_URL`, or its default. */
  readonly baseURL: string;
  /** Effective retry policy: reports whether a failure's class was retryable. */
  readonly retry: RetryPolicy;
  systemOne(request: SystemOneRequest, options?: RequestOptions): PromiseLike<unknown>;
}

export type AssessmentClientFactory = (config: TypeSafeClientConfig) => AssessmentClient;

/** SDK timeout per attempt unless `timeoutMs` is set. */
export const DEFAULT_ASSESSMENT_TIMEOUT_MS = 30_000;
/** Ceiling for `retry.maxRetries`, which the SDK leaves unbounded. */
export const MAX_ASSESSMENT_RETRIES = 10;
/** Largest delay a Node timer honors; the SDK does not check for overflow. */
const MAX_TIMER_MS = 2_147_483_647;
const ERROR_BODY_LIMIT = 2_000;

/**
 * Maximum concurrent per-file SDK calls within one batch assessment.
 * Conservative: one batch command already holds one scheduler permit, and the
 * SDK retries each attempt internally; high fan-out risks rate-limit storms.
 */
export const DEFAULT_ASSESSMENT_FILE_CONCURRENCY = 4;

export interface AssessmentExecutorOptions {
  /** Falls back to `TYPESAFE_API_KEY` in the SDK. */
  apiKey?: string;
  /** Falls back to `TYPESAFE_BASE_URL`, then `https://api.typesafe.ai`. */
  baseURL?: string;
  /** Falls back to `TYPESAFE_DEFAULT_MODEL`, then `jev-latest`; a pinned operation model wins. */
  defaultModel?: string;
  /** SDK timeout per attempt; there is no total budget. Default 30 seconds. */
  timeoutMs?: number;
  /** SDK `RetryPolicy` overrides; `maxRetries` is capped at `MAX_ASSESSMENT_RETRIES`. */
  retry?: Partial<RetryPolicy>;
  /** Default `new TypeSafeClient(config)`; tests substitute a client. */
  createClient?: AssessmentClientFactory;
  /**
   * Maximum concurrent per-file SDK calls within a single batch assessment
   * command. Clamped to `[1, 32]`. Default `DEFAULT_ASSESSMENT_FILE_CONCURRENCY` (4).
   */
  fileConcurrency?: number;
}

const silent = () => {};

export async function executeAssessment(
  options: AssessmentExecutorOptions,
  commandId: string,
  operation: AssessmentOperation,
  signal?: AbortSignal,
): Promise<OperationCompletion> {
  const base = { protocolVersion: PROTOCOL_VERSION, commandId } as const;
  const fail = (message: string, details: { [key: string]: Json }): OperationCompletion => ({
    ...base,
    status: "failed",
    error: { message, details },
  });
  const cancelled = (message: string): OperationCompletion => ({
    ...base,
    status: "cancelled",
    error: { message },
  });
  if (signal?.aborted) return cancelled("aborted before start");

  const timeout = options.timeoutMs ?? DEFAULT_ASSESSMENT_TIMEOUT_MS;
  if (timeout > MAX_TIMER_MS) {
    return fail(`timeoutMs must be at most ${MAX_TIMER_MS}`, { phase: "config" });
  }
  if ((options.retry?.maxRetries ?? 0) > MAX_ASSESSMENT_RETRIES) {
    return fail(`retry.maxRetries must be at most ${MAX_ASSESSMENT_RETRIES}`, { phase: "config" });
  }
  let client: AssessmentClient;
  try {
    client = (options.createClient ?? ((config) => new TypeSafeClient(config)))({
      // Blank means unset, as the SDK treats blank environment values.
      apiKey: options.apiKey?.trim() || undefined,
      baseURL: options.baseURL?.trim() || undefined,
      defaultModel: options.defaultModel?.trim() || undefined,
      timeout,
      retry: options.retry,
      logLevel: "off",
      logger: { debug: silent, info: silent, warn: silent, error: silent },
    });
  } catch (error) {
    // The SDK throws `TypeSafeError` for a missing API key or invalid settings.
    return fail(`assessment client configuration failed: ${messageOf(error)}`, {
      phase: "config",
    });
  }
  const urlProblem = baseUrlProblem(client.baseURL);
  if (urlProblem !== undefined) return fail(urlProblem, { phase: "config" });

  let body: unknown;
  try {
    body = await client.systemOne(
      {
        state: operation.state,
        // Validated by `questionsProblem`, which is stricter than the SDK's check.
        questions: operation.questions as unknown as Questions,
        // Undefined leaves the client's default model.
        model: operation.model,
      },
      { signal },
    );
  } catch (error) {
    if (signal?.aborted || error instanceof APIUserAbortError) {
      return cancelled("aborted during the request");
    }
    return fail(...failureOf(error, client.retry));
  }
  if (signal?.aborted) return cancelled("aborted during the request");

  const output = normalize(body);
  const problem = assessmentResultProblem(operation.questions, output);
  if (problem !== undefined) {
    return fail(`TypeSafe API returned an invalid assessment: ${problem}`, { phase: "response" });
  }
  return { ...base, status: "succeeded", output: output as Json };
}

/** The SDK does not check its base URL; a bad one would surface only as retried connection errors. */
function baseUrlProblem(baseURL: string): string | undefined {
  let problem: string | undefined;
  try {
    const url = new URL(baseURL);
    if (url.protocol !== "https:" && url.protocol !== "http:") problem = "not http(s)";
    else if (url.search !== "" || url.hash !== "") problem = "has a query or fragment";
  } catch (error) {
    problem = messageOf(error);
  }
  return problem && `assessment base URL is invalid (${problem}): ${JSON.stringify(baseURL)}`;
}

/** SDK error classes to stable failure messages and details; messages are never parsed. */
function failureOf(error: unknown, retry: RetryPolicy): [string, { [key: string]: Json }] {
  if (error instanceof APITimeoutError) {
    return [
      `assessment request timed out after ${error.timeoutMs}ms`,
      { phase: "request", timedOut: true, retryable: retry.apiTimeoutError },
    ];
  }
  if (error instanceof APIConnectionError) {
    return [
      `assessment request failed: ${error.message}`,
      { phase: "request", retryable: retry.apiConnectionError },
    ];
  }
  if (!(error instanceof APIError)) {
    return [`assessment failed unexpectedly: ${messageOf(error)}`, { phase: "internal" }];
  }
  const details: { [key: string]: Json } = {
    phase: "http",
    status: error.status,
    retryable: retry.httpStatuses.has(error.status),
  };
  if (error.requestId !== undefined) details.requestId = error.requestId;
  if (error instanceof RateLimitError && error.retryAfterMs !== undefined) {
    details.retryAfterMs = error.retryAfterMs;
  }
  if (error.body !== undefined) {
    // The SDK decodes error bodies as parsed JSON or text.
    const text = typeof error.body === "string" ? error.body : JSON.stringify(error.body);
    details.body =
      text.length > ERROR_BODY_LIMIT ? `${text.slice(0, ERROR_BODY_LIMIT)}…` : (error.body as Json);
  }
  return [`TypeSafe API responded ${error.status}`, details];
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

// ---------------------------------------------------------------------------
// File-oriented batch assessment (execution layer)
// ---------------------------------------------------------------------------

const fileDecoder = new TextDecoder("utf-8", { fatal: true });

/**
 * Read a selected file with strict UTF-8. Returns `undefined` ONLY for a
 * genuine `ENOENT` race (file vanished between selection and read, matching
 * JSSG behavior). Invalid UTF-8 and all other I/O errors throw — the caller
 * must turn them into a deterministic failed completion naming the file.
 */
function readAssessmentFile(absolute: string): string | undefined {
  try {
    return fileDecoder.decode(readFileSync(absolute));
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return undefined;
    }
    throw error;
  }
}

// Re-export from core for backward compatibility of the public API surface.
export type { AssessmentAskFn } from "../core/assessment.ts";
export { getAssessmentAsk } from "../core/assessment.ts";

/**
 * File-oriented assessment: select files from `cwd` (the `--target` root),
 * read each one with strict UTF-8, call `ask` per file to resolve questions,
 * and make one SDK call per file through `executeAssessment`. This is the
 * execution-layer counterpart of `assessment()` in the authoring layer, just
 * as `executeJssg` is the counterpart of `jssg()`.
 *
 * File selection uses the same walker as JSSG: gitignore semantics, symlinks
 * skipped, hidden files visited, component-wise deterministic order.
 *
 * Cancellation: the first per-file failure or abort cancels all remaining SDK
 * calls. The aggregate result is the first failure.
 */
export async function executeFileAssessment(
  options: AssessmentExecutorOptions,
  cwd: string,
  commandId: string,
  operation: AssessmentOperation,
  ask: AssessmentAskFn,
  signal?: AbortSignal,
): Promise<OperationCompletion> {
  const base = { protocolVersion: PROTOCOL_VERSION, commandId } as const;
  const fail = (message: string, details?: { [key: string]: Json }): OperationCompletion => ({
    ...base,
    status: "failed",
    error: details !== undefined ? { message, details } : { message },
  });
  const cancelled = (message: string): OperationCompletion => ({
    ...base,
    status: "cancelled",
    error: { message },
  });

  if (signal?.aborted) return cancelled("aborted before start");

  // Extract batch parameters from the operation's state.
  const state = operation.state as { [key: string]: unknown };
  const include = state.include as string[] | undefined;
  const exclude = state.exclude as string[] | undefined;
  const input = state.input;

  if (!Array.isArray(include) || include.length === 0) {
    return fail("file assessment state must contain a non-empty include array");
  }

  // Select files from the target root (like JSSG).
  let targetRoot: string;
  let filePaths: string[];
  try {
    targetRoot = realpathSync.native(resolve(cwd));
    filePaths = selectFiles({
      cwd: targetRoot,
      targetRoot,
      language: "",
      definition: { include, exclude },
      invocation: {},
    });
  } catch (error) {
    return fail(`file selection failed: ${(error as Error).message}`, { phase: "select" });
  }

  if (filePaths.length === 0) {
    return { ...base, status: "succeeded", output: [] as unknown as Json };
  }

  // Read files with strict UTF-8. ENOENT (race) is skipped; invalid UTF-8
  // and other I/O errors are a deterministic failure naming the file.
  const fileEntries: { path: string; content: string }[] = [];
  for (const path of filePaths) {
    let content: string | undefined;
    try {
      content = readAssessmentFile(join(targetRoot, path));
    } catch (error) {
      const phase = error instanceof TypeError ? "utf8" : "read";
      const detail =
        error instanceof TypeError
          ? `file '${path}' is not valid UTF-8`
          : `reading '${path}' failed: ${messageOf(error)}`;
      return fail(detail, { phase, file: path });
    }
    if (content !== undefined) fileEntries.push({ path, content });
  }

  if (fileEntries.length === 0) {
    return { ...base, status: "succeeded", output: [] as unknown as Json };
  }

  if (signal?.aborted) return cancelled("aborted after reading files");

  // Bounded-concurrency worker pool with sibling cancellation.
  const concurrency = Math.max(
    1,
    Math.min(32, options.fileConcurrency ?? DEFAULT_ASSESSMENT_FILE_CONCURRENCY),
  );
  const perFileController = new AbortController();
  const onAbort = () => perFileController.abort();
  if (signal) signal.addEventListener("abort", onAbort, { once: true });

  type PerFileOk = { ok: true; file: string; questions: AssessmentQuestions; assessment: Json };
  type PerFileFail = { ok: false; completion: OperationCompletion };
  type PerFileResult = PerFileOk | PerFileFail;

  // Indexed slots preserve selector order regardless of completion order.
  const results: (PerFileResult | undefined)[] = Array.from({ length: fileEntries.length });
  let nextIndex = 0;
  let firstFailure: OperationCompletion | undefined;

  async function processFile(index: number): Promise<void> {
    const file = fileEntries[index]!;

    if (perFileController.signal.aborted) {
      results[index] = { ok: false, completion: cancelled(`cancelled before '${file.path}'`) };
      return;
    }

    let questions: AssessmentQuestions;
    try {
      const raw = ask({ file, input });
      if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
        throw new Error("ask must return a questions object");
      }
      const problem = questionsProblem(raw);
      if (problem !== undefined) throw new Error(problem);
      questions = raw as AssessmentQuestions;
    } catch (error) {
      perFileController.abort();
      results[index] = {
        ok: false,
        completion: fail(`questions for '${file.path}': ${messageOf(error)}`),
      };
      return;
    }

    const perFileState: { [key: string]: Json } = {
      file: { path: file.path, content: file.content } as unknown as Json,
    };
    if (input !== undefined) perFileState.input = input as Json;

    const perFileOp: AssessmentOperation = {
      kind: "assessment",
      state: perFileState,
      questions,
      model: operation.model,
    };

    const fileCommandId = fileEntries.length === 1 ? commandId : `${commandId}:${file.path}`;
    const completion = await executeAssessment(
      options,
      fileCommandId,
      perFileOp,
      perFileController.signal,
    );

    if (completion.status !== "succeeded") {
      perFileController.abort();
      results[index] = { ok: false, completion };
      return;
    }

    results[index] = { ok: true, file: file.path, questions, assessment: completion.output };
  }

  async function worker(): Promise<void> {
    while (!perFileController.signal.aborted) {
      const index = nextIndex++;
      if (index >= fileEntries.length) break;
      await processFile(index);
    }
  }

  // Launch at most `concurrency` workers; each pulls from the shared index.
  const workerCount = Math.min(concurrency, fileEntries.length);
  await Promise.allSettled(Array.from({ length: workerCount }, () => worker()));
  if (signal) signal.removeEventListener("abort", onAbort);

  // Collect results in selector order.
  const output: PerFileOk[] = [];
  for (const result of results) {
    if (result === undefined) continue;
    if (!result.ok) {
      if (firstFailure === undefined) firstFailure = result.completion;
      continue;
    }
    output.push(result);
  }

  if (firstFailure !== undefined) return { ...firstFailure, commandId };

  return {
    ...base,
    status: "succeeded",
    output: output.map((r) => ({
      file: r.file,
      questions: r.questions,
      assessment: r.assessment,
    })) as unknown as Json,
  };
}

// ---------------------------------------------------------------------------
// SDK response normalization (shared by single and batch paths)
// ---------------------------------------------------------------------------

/**
 * Keep the documented fields only: the model, each answer's typed fields, and
 * the token counts the API reported, in the protocol's camelCase. Fields the
 * API adds later are dropped so history stays within the contract. Entries are
 * defined, never assigned, so response keys cannot reach `Object.prototype`.
 */
function normalize(value: unknown): unknown {
  if (!isRecord(value) || !isRecord(value.answers)) return value;
  if (value.usage != null && !isRecord(value.usage)) return value;
  const answers = Object.entries(value.answers).map(([id, answer]) => {
    if (!isRecord(answer) || !Object.hasOwn(ANSWER_FIELDS, String(answer.type))) {
      return [id, answer];
    }
    const fields = ANSWER_FIELDS[answer.type as keyof typeof ANSWER_FIELDS];
    return [
      id,
      Object.fromEntries(fields.filter((f) => Object.hasOwn(answer, f)).map((f) => [f, answer[f]])),
    ];
  });
  const usage = (value.usage ?? {}) as Record<string, unknown>;
  const counts = [
    ["inputTokens", usage.input_tokens],
    ["outputTokens", usage.output_tokens],
  ].filter(([, count]) => count != null);
  return {
    model: value.model,
    answers: Object.fromEntries(answers),
    usage: Object.fromEntries(counts),
  };
}

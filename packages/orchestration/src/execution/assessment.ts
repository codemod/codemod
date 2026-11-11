/**
 * Assessment executor: one `assessment` operation becomes one request to the
 * decision API the environment names (`core/decision.ts`), through that
 * provider's adapter (`decision/`), in the trusted host process. Keys and
 * endpoints come from the environment only, never from workflow code.
 *
 * The decoded response is validated against the questions, so history holds
 * only results of the documented shape. A failed assessment cannot have
 * changed the repository, so errors are `failed` and aborts `cancelled`,
 * never `unknown`.
 */
import {
  assessmentResultProblem,
  getAssessmentAsk,
  questionsProblem,
  type AssessmentAskFn,
  type AssessmentQuestions,
} from "../core/assessment.ts";
import { decisionSettings, type DecisionSettings } from "../core/decision.ts";
import { getHost } from "../core/host.ts";
import type { Json } from "../core/json.ts";
import {
  PROTOCOL_VERSION,
  type AssessmentOperation,
  type OperationCompletion,
  type OperationRequest,
} from "../core/protocol.ts";
import { DECISION_ADAPTERS } from "./decision/adapter.ts";
import {
  DEFAULT_RETRY,
  DecisionAbortError,
  DecisionRequestError,
  messageOf,
  postJson,
  type DecisionFetch,
  type RetryPolicy,
} from "./decision/transport.ts";
import { selectFiles } from "./files.ts";

/** Timeout per attempt unless `timeoutMs` is set. */
export const DEFAULT_ASSESSMENT_TIMEOUT_MS = 30_000;
/** Ceiling for `retry.maxRetries`. */
export const MAX_ASSESSMENT_RETRIES = 10;
/** Largest delay a timer honors. */
const MAX_TIMER_MS = 2_147_483_647;

/**
 * Maximum concurrent per-file requests within one batch assessment.
 * Conservative: one batch command already holds one scheduler permit, and
 * each request retries on its own; high fan-out risks rate-limit storms.
 */
export const DEFAULT_ASSESSMENT_FILE_CONCURRENCY = 4;

export interface AssessmentExecutorOptions {
  /** Where `DECISION_*` settings and keys are read. Default: the host environment. */
  env?: Readonly<Record<string, string | undefined>>;
  /** Default: the global `fetch`. The QuickJS host passes one backed by `httpRequest`. */
  fetch?: DecisionFetch;
  /** Timeout per attempt; there is no total budget. Default 30 seconds. */
  timeoutMs?: number;
  /** Overrides of the retry policy; `maxRetries` is capped at `MAX_ASSESSMENT_RETRIES`. */
  retry?: Partial<RetryPolicy>;
  /**
   * Maximum concurrent per-file requests within a single batch assessment
   * command. Clamped to `[1, 32]`. Default `DEFAULT_ASSESSMENT_FILE_CONCURRENCY` (4).
   */
  fileConcurrency?: number;
}

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

  const timeoutMs = options.timeoutMs ?? DEFAULT_ASSESSMENT_TIMEOUT_MS;
  if (!(timeoutMs >= 1 && timeoutMs <= MAX_TIMER_MS)) {
    return fail(`timeoutMs must be between 1 and ${MAX_TIMER_MS}`, { phase: "config" });
  }
  const retry = { ...DEFAULT_RETRY, ...options.retry };
  if (retry.maxRetries > MAX_ASSESSMENT_RETRIES) {
    return fail(`retry.maxRetries must be at most ${MAX_ASSESSMENT_RETRIES}`, { phase: "config" });
  }
  let settings: DecisionSettings;
  try {
    settings = decisionSettings(options.env ?? getHost().env(), operation.provider);
  } catch (error) {
    return fail(messageOf(error), { phase: "config" });
  }
  if (settings.apiKey === undefined) {
    return fail(`no API key for the ${settings.provider} decision API: set DECISION_API_KEY`, {
      phase: "config",
    });
  }
  const fetch = options.fetch ?? (globalThis.fetch as unknown as DecisionFetch | undefined);
  if (fetch === undefined) return fail("no fetch implementation", { phase: "config" });

  const adapter = DECISION_ADAPTERS[operation.provider];
  const request = {
    state: operation.state,
    questions: operation.questions,
    model: operation.model,
  };
  const encoded = adapter.encode(request);
  if ("problem" in encoded) return fail(encoded.problem, { phase: "request" });

  let body: unknown;
  try {
    body = await postJson({
      url: settings.url,
      headers: {
        [settings.authHeader]:
          settings.authHeader === "authorization" ? `Bearer ${settings.apiKey}` : settings.apiKey,
      },
      body: encoded.body,
      fetch,
      timeoutMs,
      retry,
      ...(signal === undefined ? {} : { signal }),
    });
  } catch (error) {
    if (error instanceof DecisionAbortError) return cancelled("aborted during the request");
    if (error instanceof DecisionRequestError) return fail(error.message, error.details);
    return fail(`assessment failed unexpectedly: ${messageOf(error)}`, { phase: "internal" });
  }
  if (signal?.aborted) return cancelled("aborted during the request");

  const decoded = adapter.decode(body, request);
  const output =
    "problem" in decoded
      ? undefined
      : {
          provider: operation.provider,
          model: decoded.model,
          calibrated: settings.calibrated,
          answers: decoded.answers,
          usage: decoded.usage,
        };
  const problem =
    "problem" in decoded ? decoded.problem : assessmentResultProblem(operation.questions, output);
  if (problem !== undefined) {
    return fail(`decision API returned an invalid assessment: ${problem}`, { phase: "response" });
  }
  return { ...base, status: "succeeded", output: output as unknown as Json };
}

// ---------------------------------------------------------------------------
// File-oriented batch assessment (execution layer)
// ---------------------------------------------------------------------------

/**
 * Read a selected file with strict UTF-8. Returns `undefined` ONLY for a
 * genuine `ENOENT` race (file vanished between selection and read, matching
 * JSSG behavior). Invalid UTF-8 and all other I/O errors throw — the caller
 * must turn them into a deterministic failed completion naming the file.
 */
function readAssessmentFile(absolute: string): string | undefined {
  try {
    return getHost().fs.readText(absolute, { fatal: true });
  } catch (error) {
    if (errorCode(error) === "ENOENT") return undefined;
    throw error;
  }
}

function errorCode(error: unknown): unknown {
  return typeof error === "object" && error !== null && "code" in error ? error.code : undefined;
}

/**
 * One `assessment` operation as a runtime executes it: file-oriented when the
 * operation carries its per-file question resolver (`assessment()` steps),
 * else a single request.
 */
export function executeAssessmentOperation(
  options: AssessmentExecutorOptions,
  cwd: string,
  request: OperationRequest,
  signal?: AbortSignal,
): Promise<OperationCompletion> {
  const operation = request.operation as AssessmentOperation;
  const ask = getAssessmentAsk(operation);
  return ask === undefined
    ? executeAssessment(options, request.commandId, operation, signal)
    : executeFileAssessment(options, cwd, request.commandId, operation, ask, signal);
}

// Re-export from core for backward compatibility of the public API surface.
export type { AssessmentAskFn } from "../core/assessment.ts";
export { getAssessmentAsk } from "../core/assessment.ts";

/**
 * File-oriented assessment: select files from `cwd` (the `--target` root),
 * read each one with strict UTF-8, call `ask` per file to resolve questions,
 * and make one request per file through `executeAssessment`. This is the
 * execution-layer counterpart of `assessment()` in the authoring layer, just
 * as `executeJssg` is the counterpart of `jssg()`.
 *
 * File selection uses the same walker as JSSG: gitignore semantics, symlinks
 * skipped, hidden files visited, component-wise deterministic order.
 *
 * Cancellation: the first per-file failure or abort cancels all remaining
 * requests. The aggregate result is the first failure.
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
    const host = getHost();
    targetRoot = host.fs.realpath(host.path.resolve(cwd));
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
      content = readAssessmentFile(getHost().path.join(targetRoot, path));
    } catch (error) {
      const invalidUtf8 = errorCode(error) === "EILSEQ";
      const phase = invalidUtf8 ? "utf8" : "read";
      const detail = invalidUtf8
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
      provider: operation.provider,
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

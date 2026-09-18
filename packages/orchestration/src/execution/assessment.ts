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
import { ANSWER_FIELDS, assessmentResultProblem } from "../core/assessment.ts";
import type { Json } from "../core/json.ts";
import {
  PROTOCOL_VERSION,
  isRecord,
  type AssessmentOperation,
  type OperationCompletion,
} from "../core/protocol.ts";

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

/**
 * One JSON POST to a decision API, with a per-attempt timeout, retries and
 * abort. Ported from the policy the TypeSafe SDK used, so behavior did not
 * change when the SDK was dropped: two retries with jittered exponential
 * backoff (500 ms doubling to 5 s), on connection errors, timeouts, and HTTP
 * 408, 409, 429 and 5xx, waiting for `retry-after-ms` / `Retry-After` when the
 * server sends it (up to 60 s).
 *
 * Failures are thrown as `DecisionRequestError`, whose `phase` and `details`
 * become the failed completion's; an abort is thrown as `DecisionAbortError`.
 */
import type { Json } from "../../core/json.ts";
import { getHost } from "../../core/host.ts";

/** The part of `fetch` used here; the QuickJS host passes its own. */
export type DecisionFetch = (
  url: string,
  init: { method: string; headers: Record<string, string>; body: string; signal: AbortSignal },
) => Promise<{
  status: number;
  headers: { get(name: string): string | null };
  text(): Promise<string>;
}>;

export interface RetryPolicy {
  maxRetries: number;
  initialDelayMs: number;
  maxDelayMs: number;
  /** Fraction of each delay drawn at random, 0 to 1. */
  jitter: number;
}

export const DEFAULT_RETRY: RetryPolicy = {
  maxRetries: 2,
  initialDelayMs: 500,
  maxDelayMs: 5_000,
  jitter: 0.25,
};
const MAX_RETRY_AFTER_MS = 60_000;
const ERROR_BODY_LIMIT = 2_000;

export class DecisionRequestError extends Error {
  constructor(
    message: string,
    readonly details: { [key: string]: Json },
  ) {
    super(message);
  }
}

export class DecisionAbortError extends Error {}

export interface PostOptions {
  url: string;
  headers: Record<string, string>;
  body: unknown;
  fetch: DecisionFetch;
  timeoutMs: number;
  retry: RetryPolicy;
  signal?: AbortSignal;
}

const retryableStatus = (status: number) =>
  status === 408 || status === 409 || status === 429 || status >= 500;

/** POST `body` as JSON and return the parsed 2xx response body. */
export async function postJson(options: PostOptions): Promise<unknown> {
  const body = JSON.stringify(options.body);
  for (let attempt = 0; ; attempt++) {
    let retryAfterMs: number | undefined;
    try {
      const response = await attemptOnce(options, body);
      const text = await response.text();
      if (response.status >= 200 && response.status < 300) {
        try {
          return JSON.parse(text) as unknown;
        } catch {
          throw new DecisionRequestError("decision API returned a body that is not JSON", {
            phase: "response",
            status: response.status,
          });
        }
      }
      retryAfterMs = retryAfter(response.headers);
      throw new DecisionRequestError(`decision API responded ${response.status}`, {
        phase: "http",
        status: response.status,
        retryable: retryableStatus(response.status),
        ...httpDetails(response.headers, text, retryAfterMs),
      });
    } catch (error) {
      if (error instanceof DecisionAbortError) throw error;
      const failure =
        error instanceof DecisionRequestError
          ? error
          : new DecisionRequestError(`decision request failed: ${messageOf(error)}`, {
              phase: "internal",
            });
      if (failure.details.retryable !== true || attempt >= options.retry.maxRetries) {
        throw failure;
      }
    }
    await sleep(retryAfterMs ?? backoff(options.retry, attempt), options.signal);
  }
}

async function attemptOnce(options: PostOptions, body: string) {
  const { signal } = options;
  if (signal?.aborted) throw new DecisionAbortError("aborted");
  const host = getHost();
  const controller = new AbortController();
  let timedOut = false;
  const timer = host.setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, options.timeoutMs);
  const onAbort = () => controller.abort();
  signal?.addEventListener("abort", onAbort, { once: true });
  try {
    return await options.fetch(options.url, {
      method: "POST",
      headers: {
        ...options.headers,
        accept: "application/json",
        "content-type": "application/json",
      },
      body,
      signal: controller.signal,
    });
  } catch (error) {
    if (signal?.aborted) throw new DecisionAbortError("aborted");
    if (timedOut) {
      throw new DecisionRequestError(`decision request timed out after ${options.timeoutMs}ms`, {
        phase: "request",
        timedOut: true,
        retryable: true,
      });
    }
    throw new DecisionRequestError(`decision request failed: ${messageOf(error)}`, {
      phase: "request",
      retryable: true,
    });
  } finally {
    host.clearTimeout(timer);
    signal?.removeEventListener("abort", onAbort);
  }
}

function httpDetails(
  headers: { get(name: string): string | null },
  text: string,
  retryAfterMs: number | undefined,
): { [key: string]: Json } {
  const details: { [key: string]: Json } = {};
  const requestId = headers.get("x-request-id") ?? headers.get("apim-request-id");
  if (requestId !== null) details.requestId = requestId;
  if (retryAfterMs !== undefined) details.retryAfterMs = retryAfterMs;
  if (text !== "") {
    let parsed: Json = text;
    try {
      parsed = JSON.parse(text) as Json;
    } catch {}
    details.body = text.length > ERROR_BODY_LIMIT ? `${text.slice(0, ERROR_BODY_LIMIT)}…` : parsed;
  }
  return details;
}

function retryAfter(headers: { get(name: string): string | null }): number | undefined {
  const milliseconds = Number(headers.get("retry-after-ms") ?? Number.NaN);
  let delay = Number.isFinite(milliseconds) ? milliseconds : undefined;
  const header = headers.get("retry-after");
  if (delay === undefined && header !== null) {
    const seconds = Number(header);
    delay = Number.isFinite(seconds) ? seconds * 1000 : Date.parse(header) - Date.now();
    if (!Number.isFinite(delay)) delay = undefined;
  }
  return delay !== undefined && delay >= 0 && delay <= MAX_RETRY_AFTER_MS ? delay : undefined;
}

function backoff(policy: RetryPolicy, attempt: number): number {
  const delay = Math.min(policy.maxDelayMs, policy.initialDelayMs * 2 ** attempt);
  return delay * (1 - policy.jitter * Math.random());
}

function sleep(ms: number, signal: AbortSignal | undefined): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(new DecisionAbortError("aborted"));
    const host = getHost();
    const onAbort = () => {
      host.clearTimeout(timer);
      reject(new DecisionAbortError("aborted"));
    };
    const timer = host.setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

export function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * The `fetch` assessments get inside the QuickJS host
 * (`AssessmentExecutorOptions.fetch`), built on the host's `httpRequest`.
 * The host refuses any URL outside the configured decision API root
 * (`crates/orchestration/src/host/http.rs`); there is no global
 * `fetch` in the context, so workflow and transform code get no network.
 *
 * Only what the decision transport uses is implemented:
 * `init.method/headers/body/signal` (headers as a plain object, body a
 * string) and a response with `ok`, `status`, `headers.get`, `text()`,
 * `json()`, and `clone()`. The body is buffered by the host. Abort rejects at
 * once with an `AbortError`; the host's request thread ends on its own within
 * its timeout.
 */
import type { QuickjsNative } from "./native.ts";

/** Upper bound on one request in the host; the assessment timeout aborts sooner. */
const HOST_TIMEOUT_MS = 10 * 60 * 1000;

class BufferedHeaders {
  readonly #entries: Map<string, string>;

  constructor(entries: readonly (readonly [string, string])[]) {
    this.#entries = new Map();
    for (const [name, value] of entries) {
      const key = name.toLowerCase();
      const previous = this.#entries.get(key);
      this.#entries.set(key, previous === undefined ? value : `${previous}, ${value}`);
    }
  }

  get(name: string): string | null {
    return this.#entries.get(name.toLowerCase()) ?? null;
  }

  has(name: string): boolean {
    return this.#entries.has(name.toLowerCase());
  }

  forEach(callback: (value: string, name: string) => void): void {
    for (const [name, value] of this.#entries) callback(value, name);
  }
}

class BufferedResponse {
  readonly ok: boolean;
  readonly body = undefined;

  constructor(
    readonly status: number,
    readonly headers: BufferedHeaders,
    readonly url: string,
    readonly rawHeaders: readonly (readonly [string, string])[],
    readonly bodyText: string,
  ) {
    this.ok = status >= 200 && status < 300;
  }

  async text(): Promise<string> {
    return this.bodyText;
  }

  async json(): Promise<unknown> {
    return JSON.parse(this.bodyText);
  }

  clone(): BufferedResponse {
    return new BufferedResponse(
      this.status,
      this.headers,
      this.url,
      this.rawHeaders,
      this.bodyText,
    );
  }
}

function abortError(signal: AbortSignal): unknown {
  if (signal.reason !== undefined) return signal.reason;
  const error = new Error("The operation was aborted.");
  error.name = "AbortError";
  return error;
}

interface FetchInit {
  method?: string;
  headers?: Record<string, string>;
  body?: string;
  signal?: AbortSignal;
}

export function createQuickjsFetch(native: QuickjsNative) {
  return async (input: string | { toString(): string }, init: FetchInit = {}) => {
    const { signal } = init;
    if (signal?.aborted) throw abortError(signal);
    if (init.body !== undefined && typeof init.body !== "string") {
      throw new TypeError("the QuickJS host fetch only sends string bodies");
    }
    const url = String(input);
    const pending = native.httpRequest({
      url,
      method: init.method ?? "GET",
      headers: { ...init.headers },
      ...(init.body === undefined ? {} : { body: init.body }),
      timeoutMs: HOST_TIMEOUT_MS,
    });
    const response = await new Promise<Awaited<typeof pending>>((resolve, reject) => {
      const onAbort = () => reject(abortError(signal!));
      signal?.addEventListener("abort", onAbort, { once: true });
      pending.then(
        (value) => {
          signal?.removeEventListener("abort", onAbort);
          resolve(value);
        },
        (error: unknown) => {
          signal?.removeEventListener("abort", onAbort);
          reject(error);
        },
      );
    });
    return new BufferedResponse(
      response.status,
      new BufferedHeaders(response.headers),
      url,
      response.headers,
      response.body,
    );
  };
}

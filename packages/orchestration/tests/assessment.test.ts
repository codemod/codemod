/**
 * Mocked contract tests for `assessment()` and its TypeSafe SDK adapter. No
 * test reaches the network: executor tests inject either a fake
 * `AssessmentClient` that throws real SDK error instances, or the real
 * `TypeSafeClient` over a test-only transport. The live check is
 * `assessment.live.test.ts`.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  APIConnectionError,
  APITimeoutError,
  APIUserAbortError,
  AuthenticationError,
  InternalServerError,
  RateLimitError,
  TypeSafeClient,
  UnprocessableEntityError,
  VERSION,
  type RequestOptions,
  type SystemOneRequest,
  type TypeSafeClientConfig,
} from "@typesafe-ai/sdk";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createHarness } from "../src/host/harness.ts";
import {
  BridgeExecutor,
  CollectingSink,
  DEFAULT_ASSESSMENT_TIMEOUT_MS,
  MAX_ASSESSMENT_RETRIES,
  MemoryHistoryStore,
  OperationError,
  PROTOCOL_VERSION,
  assessment,
  canonicalJson,
  dynamic,
  executeAssessment,
  guard,
  isOperation,
  isOperationCompletion,
  isOperationRequest,
  run,
  sequence,
  shell,
  type AssessmentClient,
  type AssessmentClientFactory,
  type AssessmentExecutorOptions,
  type AssessmentOperation,
  type Json,
  type OperationCompletion,
} from "../src/index.ts";

const dir = join(import.meta.dirname, "..", "fixtures", "protocol");
const fixture = <T = Record<string, unknown>>(name: string) =>
  JSON.parse(readFileSync(join(dir, name), "utf8")) as T;

interface Review {
  package: string;
  diff: string;
}
const Review = guard(
  "review",
  (v): v is Review => typeof v === "object" && v !== null && "package" in v && "diff" in v,
);

const reviewQuestions = {
  risk: {
    type: "choice" as const,
    instructions: "How risky is this change to merge without review?",
    criteria: {
      low: "Mechanical, behavior-preserving",
      medium: null,
      high: "Could change runtime behavior",
    },
  },
  completeness: {
    type: "score" as const,
    instructions: "How completely does the diff perform the migration?",
    criteria: ["Not started", "Partial", "Complete"],
  },
  touchesTests: {
    type: "noul" as const,
    instructions: "Does the diff modify test files?",
    criteria: { true: "At least one test file changed", false: "No test files changed" },
  },
};

const reviewDiff = assessment({
  name: "review-diff",
  input: Review,
  ask: (review) => ({
    state: { package: review.package, diff: review.diff },
    questions: reviewQuestions,
  }),
  model: "jev-latest",
});

const review: Review = {
  package: "web",
  diff: "- import { a } from 'old'\n+ import { a } from 'new'",
};
const operation = reviewDiff.toOperation(review);
const expectedOutput = fixture<{ output: Json }>("assessment-completion.json").output;

/** The documented response body for the fixture request, plus a field the contract drops. */
const apiResponse = {
  model: "jev-1.13.0",
  answers: {
    risk: {
      type: "choice",
      choice: "low",
      probabilities: { low: 0.86, medium: 0.1, high: 0.04 },
      confidence: 0.81,
    },
    completeness: {
      type: "score",
      score: 1.9,
      legend: { "0": "Not started", "1": "Partial", "2": "Complete" },
      probabilities: { "0": 0.02, "1": 0.06, "2": 0.92 },
      confidence: 0.87,
    },
    touchesTests: { type: "noul", noul: 0.03 },
  },
  usage: { input_tokens: 312, output_tokens: 48 },
  request_id: "req_123",
};

/** The SDK's default retry policy, read from a client that never sends anything. */
const SDK_RETRY = new TypeSafeClient({ apiKey: "unused", logLevel: "off" }).retry;

/** A fake `AssessmentClient` recording configuration and calls, answering with `respond`. */
function fakeClient(
  respond: () => unknown = () => apiResponse,
  overrides: Partial<AssessmentClient> = {},
) {
  const configs: TypeSafeClientConfig[] = [];
  const calls: { request: SystemOneRequest; options?: RequestOptions }[] = [];
  const createClient: AssessmentClientFactory = (config) => {
    configs.push(config);
    return {
      baseURL: "https://api.typesafe.ai",
      retry: SDK_RETRY,
      systemOne: async (request, options) => {
        calls.push({ request, options });
        return respond();
      },
      ...overrides,
    };
  };
  return { configs, calls, createClient };
}

interface WireCall {
  url: string;
  init: RequestInit;
}

/** The real SDK client over a test-only transport: its own encoding, retries, timeouts, and aborts. */
function sdkClient(respond: (call: WireCall, index: number) => Response | Promise<Response>) {
  const wire: WireCall[] = [];
  const createClient: AssessmentClientFactory = (config) =>
    new TypeSafeClient({
      ...config,
      fetch: async (url, init) => {
        wire.push({ url, init: init ?? {} });
        return respond(wire.at(-1)!, wire.length - 1);
      },
    });
  return { wire, createClient };
}

/** A transport that never answers and rejects only when the SDK aborts the attempt. */
const hang = (call: WireCall) =>
  new Promise<Response>((_, reject) =>
    call.init.signal?.addEventListener("abort", () => reject(call.init.signal?.reason)),
  );

const execute = (options: AssessmentExecutorOptions, signal?: AbortSignal) =>
  executeAssessment({ apiKey: "test-key", ...options }, "review-diff", operation, signal);

/** Set exactly these `TYPESAFE_*` variables; the SDK treats blank values as unset. */
function useEnv(values: Record<string, string> = {}) {
  for (const name of ["API_KEY", "BASE_URL", "DEFAULT_MODEL", "LOG_LEVEL"]) {
    vi.stubEnv(`TYPESAFE_${name}`, values[`TYPESAFE_${name}`] ?? "");
  }
}

/** `apiResponse` with some answers replaced. */
const withAnswers = (answers: Record<string, unknown>) => ({
  ...apiResponse,
  answers: { ...apiResponse.answers, ...answers },
});

const fast = { backoffInitialMs: 0, backoffMaxMs: 0 };

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
});

describe("assessment authoring and decoding", () => {
  it("builds the shared fixture operation and omits an unpinned model", () => {
    const request = { protocolVersion: PROTOCOL_VERSION, commandId: "review-diff", operation };
    expect(isOperationRequest(request)).toBe(true);
    expect(canonicalJson(request)).toBe(canonicalJson(fixture("assessment-request.json")));
    expect(reviewDiff.kind).toBe("assessment");
    expect(reviewDiff.ask(review).questions).toEqual(operation.questions);
    const questions = { isUrgent: { type: "noul", instructions: "Does this convey urgency?" } };
    const plain = assessment({
      name: "urgent",
      ask: () => ({ state: "Payouts failing!", questions }),
    } as never);
    expect(plain.toOperation()).toEqual({
      kind: "assessment",
      state: "Payouts failing!",
      questions,
    });
  });

  it("decodes into typed answers keyed by question id", async () => {
    const result = await reviewDiff.decode(expectedOutput, operation);
    const choice: "low" | "medium" | "high" = result.answers.risk.choice;
    const riskHigh: number = result.answers.risk.probabilities.high;
    const score: number = result.answers.completeness.score;
    const yes: number = result.answers.touchesTests.noul;
    // @ts-expect-error noul answers carry no separate confidence
    void result.answers.touchesTests.confidence;
    // @ts-expect-error only declared question ids are answered
    void result.answers.missing;
    expect([choice, riskHigh, score, yes]).toEqual(["low", 0.04, 1.9, 0.03]);
    expect(result.model).toBe("jev-1.13.0");
    expect(result.usage).toEqual({ inputTokens: 312, outputTokens: 48 });
  });

  it("rejects recorded or scripted output that does not answer the questions", async () => {
    const answers = (expectedOutput as { answers: Record<string, Record<string, Json>> }).answers;
    const withAnswer = (id: string, answer: Json) => ({
      ...(expectedOutput as object),
      answers: { ...answers, [id]: answer },
    });
    const cases: [unknown, RegExp][] = [
      [{ ...(expectedOutput as object), model: "" }, /model must be a non-empty string/],
      [{ ...(expectedOutput as object), usage: { inputTokens: -1 } }, /usage must be/],
      [{ ...(expectedOutput as object), usage: { tokens: 1 } }, /usage must be/],
      [
        { ...(expectedOutput as object), answers: { risk: answers.risk } },
        /answers must contain exactly the questions 'risk', 'completeness', 'touchesTests'/,
      ],
      [withAnswer("risk", { ...answers.risk!, choice: "none" }), /choice must be one of/],
      [
        withAnswer("risk", { ...answers.risk!, probabilities: { low: 1 } }),
        /probabilities must cover exactly the defined options/,
      ],
      [withAnswer("risk", { ...answers.risk!, confidence: 1.2 }), /confidence must be between/],
      [
        withAnswer("completeness", { ...answers.completeness!, score: 3 }),
        /score must be between 0 and 2/,
      ],
      [
        withAnswer("completeness", { ...answers.completeness!, legend: { "0": "a" } }),
        /legend must cover exactly the defined levels/,
      ],
      [withAnswer("touchesTests", { type: "noul", noul: -0.1 }), /noul must be a probability/],
      [withAnswer("touchesTests", { type: "choice", noul: 0.5 }), /must have type 'noul'/],
      [withAnswer("touchesTests", { type: "noul", noul: 0.5, extra: 1 }), /unknown fields/],
    ];
    for (const [output, message] of cases) {
      await expect(reviewDiff.decode(output as Json, operation)).rejects.toThrow(message);
    }
  });

  const noul = { type: "noul", instructions: "ok?" } as const;

  it("refuses malformed definitions when the runnable is created", () => {
    const define = (overrides: Record<string, unknown>) => () =>
      assessment({
        name: "a",
        ask: () => ({ state: "s", questions: { ok: noul } }),
        ...overrides,
      } as never);
    expect(define({})).not.toThrow();
    expect(define({ name: " " })).toThrow("assessment name must not be empty");
    expect(define({ ask: "not a function" })).toThrow("ask must be a function");
    expect(define({ model: "" })).toThrow("model must not be empty");
  });

  it("refuses malformed questions when the operation is built", () => {
    /** Build a runnable whose `ask` returns the given questions and call `toOperation`. */
    const build =
      (questions: unknown, state: unknown = "s") =>
      () =>
        assessment({
          name: "a",
          ask: () => ({ state, questions }),
        } as never).toOperation();
    expect(build({ ok: noul })).not.toThrow();
    expect(build({})).toThrow("questions must contain at least one question");
    expect(build({ "": noul })).toThrow("question ids must be non-empty");
    for (const reserved of ["__proto__", "constructor", "prototype"]) {
      expect(build(JSON.parse(`{"${reserved}":{"type":"noul","instructions":"x"}}`))).toThrow(
        `question id '${reserved}' is reserved`,
      );
      expect(
        build({
          q: {
            type: "choice",
            instructions: "x",
            criteria: JSON.parse(`{"a":null,"${reserved}":null}`),
          },
        }),
      ).toThrow(`choice option '${reserved}' is reserved`);
    }
    // Empty object and array state are valid: the live API evaluates them (verified 2026-09-17).
    expect(build({ ok: noul }, {})).not.toThrow();
    expect(build({ ok: noul }, [])).not.toThrow();
    expect(build({ q: { type: "rank", instructions: "x" } })).toThrow(
      "question 'q' type must be 'choice', 'score', or 'noul'",
    );
    expect(build({ q: { type: "noul", instructions: " " } })).toThrow(
      "question 'q' instructions must be non-empty text, a JSON object, or a JSON array",
    );
    expect(build({ q: { type: "choice", instructions: "x", criteria: { a: null } } })).toThrow(
      "choice criteria must define at least two options",
    );
    expect(build({ q: { type: "score", instructions: "x", criteria: ["only"] } })).toThrow(
      "score criteria must list at least two levels",
    );
    expect(build({ q: { type: "noul", instructions: "x", criteria: { maybe: "?" } } })).toThrow(
      "noul criteria may only describe 'true' and 'false'",
    );
    expect(build({ q: { ...noul, options: [] } })).toThrow("has unknown fields");
    expect(build({ ok: noul }, "")).toThrow("state must be non-empty text");
    expect(build({ ok: noul }, 42)).toThrow("state must be non-empty text");
    // Structured instructions and criteria are allowed (TypeSafe "Advanced: structure").
    expect(
      build({
        q: {
          type: "choice",
          instructions: { task: "classify", rules: ["one"] },
          criteria: { a: { means: "first" }, b: ["second"] },
        },
      }),
    ).not.toThrow();
  });

  it("fails an issued command whose ask function returns invalid state", async () => {
    const bad = assessment({
      name: "bad-state",
      input: Review,
      ask: () => ({ state: "", questions: { ok: noul } }),
    });
    const h = createHarness({ fallback: () => expectedOutput });
    await expect(h.run(dynamic(() => bad({ input: review })))).rejects.toThrow(
      "assessment 'bad-state': state must be non-empty text",
    );
    expect(h.executed).toHaveLength(0);
  });

  it.each<[string, unknown]>([
    ["null", null],
    ["undefined", undefined],
    ["a number", 42],
    ["a string", "oops"],
    ["an array", [{ state: "s", questions: { ok: noul } }]],
    ["an object missing state", { questions: { ok: noul } }],
    ["an object missing questions", { state: "s" }],
  ])("fails with a stable error when ask returns %s", (_label, value) => {
    const bad = assessment({
      name: "bad-ask",
      ask: () => value as never,
    });
    expect(() => bad.toOperation()).toThrow(
      "assessment 'bad-ask': ask must return { state, questions }",
    );
  });

  it("validates assessment operations strictly on the wire", () => {
    expect(isOperation(operation)).toBe(true);
    expect(isOperation({ ...operation, model: " " })).toBe(false);
    expect(isOperation({ ...operation, state: 1 })).toBe(false);
    expect(isOperation({ ...operation, questions: {} })).toBe(false);
    expect(isOperation({ ...operation, prompt: "x" })).toBe(false);
    expect(
      isOperation({
        ...operation,
        questions: { q: { type: "score", instructions: "x", criteria: [] } },
      }),
    ).toBe(false);
  });
});

describe("assessment through the TypeSafe SDK", () => {
  it("builds the client from explicit settings with logging off and passes the run's signal", async () => {
    const { configs, calls, createClient } = fakeClient();
    const retry = { maxRetries: 1, backoffInitialMs: 0 };
    const controller = new AbortController();
    const completion = await execute(
      {
        createClient,
        apiKey: " test-key ",
        baseURL: "https://gateway.test/typesafe/",
        defaultModel: "jev-preview",
        timeoutMs: 1_234,
        retry,
      },
      controller.signal,
    );
    expect(completion).toEqual({
      protocolVersion: PROTOCOL_VERSION,
      commandId: "review-diff",
      status: "succeeded",
      output: expectedOutput,
    });
    expect(isOperationCompletion(completion)).toBe(true);
    const logger = expect.objectContaining({ warn: expect.any(Function) });
    expect(configs).toEqual([
      {
        apiKey: "test-key",
        baseURL: "https://gateway.test/typesafe/",
        defaultModel: "jev-preview",
        timeout: 1_234,
        retry,
        logLevel: "off",
        logger,
      },
    ]);
    const { state, questions } = operation;
    expect(calls).toEqual([
      {
        request: { state, questions, model: "jev-latest" },
        options: { signal: controller.signal },
      },
    ]);

    // Undefined and blank settings are left to the SDK's fallbacks; an unpinned model to its default.
    const defaults = fakeClient();
    const unpinned = { ...operation, model: undefined };
    const blank = { createClient: defaults.createClient, apiKey: undefined, baseURL: "  " };
    await executeAssessment(blank, "c", unpinned);
    expect(defaults.configs).toEqual([
      { timeout: DEFAULT_ASSESSMENT_TIMEOUT_MS, logLevel: "off", logger },
    ]);
    expect(defaults.calls[0]!.request).toEqual({ state, questions, model: undefined });
  });

  it.each<
    [string, Record<string, string>, AssessmentExecutorOptions, boolean, [string, string, string]]
  >([
    [
      "explicit settings",
      {},
      { apiKey: "k", baseURL: "https://typesafe.test/" },
      true,
      ["https://typesafe.test", "k", "jev-latest"],
    ],
    [
      "TYPESAFE_* variables",
      {
        TYPESAFE_API_KEY: "env-key",
        TYPESAFE_BASE_URL: "https://env.test",
        TYPESAFE_DEFAULT_MODEL: "jev-1.13.0",
      },
      {},
      false,
      ["https://env.test", "env-key", "jev-1.13.0"],
    ],
    [
      "options over variables",
      { TYPESAFE_API_KEY: "env-key", TYPESAFE_DEFAULT_MODEL: "jev-1.13.0" },
      { apiKey: "option-key", defaultModel: "jev-preview" },
      false,
      ["https://api.typesafe.ai", "option-key", "jev-preview"],
    ],
    [
      "a pinned model over defaults",
      { TYPESAFE_API_KEY: "env-key", TYPESAFE_DEFAULT_MODEL: "jev-1.13.0" },
      {},
      true,
      ["https://api.typesafe.ai", "env-key", "jev-latest"],
    ],
    [
      "SDK defaults",
      { TYPESAFE_API_KEY: "env-key" },
      {},
      false,
      ["https://api.typesafe.ai", "env-key", "jev-latest"],
    ],
  ])(
    "sends the documented System One request with %s",
    async (_label, env, options, pinned, [root, key, model]) => {
      useEnv(env);
      const { wire, createClient } = sdkClient(() => Response.json(apiResponse));
      const op = pinned ? operation : { ...operation, model: undefined };
      expect((await executeAssessment({ ...options, createClient }, "c", op)).status).toBe(
        "succeeded",
      );
      expect(wire).toHaveLength(1);
      const [{ url, init }] = wire as [WireCall];
      const headers = new Headers(init.headers);
      expect([url, init.method, headers.get("authorization")]).toEqual([
        `${root}/v1/systemone`,
        "POST",
        `Bearer ${key}`,
      ]);
      expect(headers.get("content-type")).toBe("application/json");
      expect(headers.get("x-typesafe-sdk")).toBe(`typesafe-sdk/${VERSION}`);
      expect(JSON.parse(init.body as string)).toEqual({
        state: op.state,
        questions: op.questions,
        model,
      });
      expect(VERSION).toBe("0.6.0");
    },
  );

  it("never writes to the terminal, even with TYPESAFE_LOG_LEVEL=debug and a retry", async () => {
    const spies = (["debug", "info", "warn", "error", "log"] as const).map((level) =>
      vi.spyOn(console, level).mockImplementation(() => {}),
    );
    useEnv({ TYPESAFE_LOG_LEVEL: "debug" });
    const { wire, createClient } = sdkClient((_call, index) =>
      index === 0 ? new Response("", { status: 503 }) : Response.json(apiResponse),
    );
    expect((await execute({ createClient, retry: fast })).status).toBe("succeeded");
    expect(wire).toHaveLength(2);
    for (const spy of spies) expect(spy).not.toHaveBeenCalled();
  });

  it.each<[string, Record<string, string>, AssessmentExecutorOptions, RegExp]>([
    [
      "no API key",
      {},
      { apiKey: undefined },
      /configuration failed: No API key was provided.*TYPESAFE_API_KEY/,
    ],
    ["a NaN timeout", {}, { timeoutMs: Number.NaN }, /`timeout` must be a positive number/],
    ["a zero timeout", {}, { timeoutMs: 0 }, /`timeout` must be a positive number/],
    ["an infinite timeout", {}, { timeoutMs: Infinity }, /timeoutMs must be at most 2147483647/],
    ["an overflowing timeout", {}, { timeoutMs: 2 ** 31 }, /timeoutMs must be at most 2147483647/],
    [
      "a fractional maxRetries",
      {},
      { retry: { maxRetries: 1.5 } },
      /`retry.maxRetries` must be a non-negative integer/,
    ],
    [
      "a negative maxRetries",
      {},
      { retry: { maxRetries: -1 } },
      /`retry.maxRetries` must be a non-negative integer/,
    ],
    [
      "a NaN maxRetries",
      {},
      { retry: { maxRetries: Number.NaN } },
      /`retry.maxRetries` must be a non-negative integer/,
    ],
    [
      "maxRetries above the cap",
      {},
      { retry: { maxRetries: 11 } },
      /retry.maxRetries must be at most 10/,
    ],
    [
      "an invalid jitter",
      {},
      { retry: { backoffJitter: 2 } },
      /`retry.backoffJitter` must be between 0 and 1/,
    ],
    [
      "a relative base URL",
      {},
      { baseURL: "api.typesafe.ai" },
      /base URL is invalid .*"api.typesafe.ai"/,
    ],
    [
      "a non-http base URL",
      {},
      { baseURL: "ftp://api.typesafe.ai" },
      /base URL is invalid \(not http\(s\)\)/,
    ],
    [
      "a base URL with a query",
      {},
      { baseURL: "https://api.typesafe.ai/?x=1" },
      /has a query or fragment/,
    ],
    [
      "an invalid TYPESAFE_BASE_URL",
      { TYPESAFE_BASE_URL: "http://[::1" },
      {},
      /base URL is invalid/,
    ],
  ])(
    "returns a config failure before any request for %s",
    async (_label, env, options, message) => {
      useEnv(env);
      const { wire, createClient } = sdkClient(() => Response.json(apiResponse));
      const completion = await execute({ ...options, createClient });
      expect(completion.error).toEqual({
        message: expect.stringMatching(message),
        details: { phase: "config" },
      });
      expect(wire).toHaveLength(0);
      expect(JSON.stringify(completion)).not.toContain("test-key");
    },
  );

  it.each<[string, number[], AssessmentExecutorOptions["retry"], number]>([
    ["retries a 503 once, then succeeds", [503, 200], fast, 2],
    ["stops when maxRetries is 0", [503], { ...fast, maxRetries: 0 }, 1],
    ["retries a 503 twice by default", [503], fast, 3],
    [
      "retries up to MAX_ASSESSMENT_RETRIES",
      [503],
      { ...fast, maxRetries: MAX_ASSESSMENT_RETRIES },
      11,
    ],
    ["does not retry a 401", [401], fast, 1],
    ["does not retry a 422", [422], fast, 1],
  ])("owns retries in the SDK only: %s", async (_label, statuses, retry, calls) => {
    const { wire, createClient } = sdkClient((_call, index) => {
      const status = statuses[Math.min(index, statuses.length - 1)]!;
      return status === 200 ? Response.json(apiResponse) : new Response("", { status });
    });
    const completion = await execute({ createClient, retry });
    const last = statuses.at(-1)!;
    if (last === 200) expect(completion.status).toBe("succeeded");
    else {
      expect(completion.error?.details).toEqual({
        phase: "http",
        status: last,
        retryable: last === 503,
      });
    }
    expect(wire).toHaveLength(calls);
    // One retry layer: the SDK numbers its own attempts.
    expect(
      wire.map((call) => new Headers(call.init.headers).get("x-typesafe-retry-count")),
    ).toEqual(wire.map((_, index) => (index === 0 ? null : String(index))));
  });

  it("times out each attempt and cancels before start, during a request, and during backoff", async () => {
    const slow = sdkClient(hang);
    expect(
      (
        await execute({
          createClient: slow.createClient,
          timeoutMs: 20,
          retry: { ...fast, maxRetries: 1 },
        })
      ).error,
    ).toEqual({
      message: "assessment request timed out after 20ms",
      details: { phase: "request", timedOut: true, retryable: true },
    });
    expect(slow.wire).toHaveLength(2);

    const before = fakeClient();
    expect(await execute({ createClient: before.createClient }, AbortSignal.abort())).toMatchObject(
      {
        status: "cancelled",
        error: { message: "aborted before start" },
      },
    );
    expect(before.configs).toHaveLength(0);

    const during = new AbortController();
    const hanging = sdkClient((call) => {
      queueMicrotask(() => during.abort());
      return hang(call);
    });
    expect(await execute({ createClient: hanging.createClient }, during.signal)).toMatchObject({
      status: "cancelled",
      error: { message: "aborted during the request" },
    });

    const waiting = new AbortController();
    const backingOff = sdkClient(() => {
      setTimeout(() => waiting.abort(), 20);
      return new Response("", { status: 503 });
    });
    const started = Date.now();
    const retry = { backoffInitialMs: 10_000, backoffMaxMs: 10_000, backoffJitter: 0 };
    const stopped = await execute({ createClient: backingOff.createClient, retry }, waiting.signal);
    expect(stopped.status).toBe("cancelled");
    expect(Date.now() - started).toBeLessThan(2_000);
    expect(backingOff.wire).toHaveLength(1);
  });
});

describe("assessment failure mapping", () => {
  const noRetries = {
    retry: {
      ...SDK_RETRY,
      httpStatuses: new Set<number>(),
      apiConnectionError: false,
      apiTimeoutError: false,
    },
  };

  it.each<[string, unknown, Partial<AssessmentClient>, OperationCompletion["error"]]>([
    [
      "401",
      new AuthenticationError(
        401,
        { detail: "Invalid API key" },
        new Headers({ "x-typesafe-request-id": "req_1" }),
      ),
      {},
      {
        message: "TypeSafe API responded 401",
        details: {
          phase: "http",
          status: 401,
          retryable: false,
          requestId: "req_1",
          body: { detail: "Invalid API key" },
        },
      },
    ],
    [
      "429",
      new RateLimitError(429, "slow down", new Headers({ "retry-after": "2" })),
      {},
      {
        message: "TypeSafe API responded 429",
        details: {
          phase: "http",
          status: 429,
          retryable: true,
          retryAfterMs: 2_000,
          body: "slow down",
        },
      },
    ],
    [
      "503",
      new InternalServerError(503, undefined, new Headers()),
      {},
      {
        message: "TypeSafe API responded 503",
        details: { phase: "http", status: 503, retryable: true },
      },
    ],
    [
      "a 500 outside the retry policy",
      new InternalServerError(500, undefined, new Headers()),
      noRetries,
      {
        message: "TypeSafe API responded 500",
        details: { phase: "http", status: 500, retryable: false },
      },
    ],
    [
      "a timeout",
      new APITimeoutError(1_500),
      {},
      {
        message: "assessment request timed out after 1500ms",
        details: { phase: "request", timedOut: true, retryable: true },
      },
    ],
    [
      "a timeout outside the retry policy",
      new APITimeoutError(10),
      noRetries,
      {
        message: "assessment request timed out after 10ms",
        details: { phase: "request", timedOut: true, retryable: false },
      },
    ],
    [
      "a connection error",
      new APIConnectionError("Connection error: fetch failed"),
      {},
      {
        message: "assessment request failed: Connection error: fetch failed",
        details: { phase: "request", retryable: true },
      },
    ],
    [
      "an unexpected error",
      new Error("boom"),
      {},
      { message: "assessment failed unexpectedly: boom", details: { phase: "internal" } },
    ],
    ["an SDK abort", new APIUserAbortError(), {}, { message: "aborted during the request" }],
  ])("maps %s by SDK error class", async (label, error, overrides, expected) => {
    const completion = await execute({
      createClient: fakeClient(() => {
        throw error;
      }, overrides).createClient,
    });
    expect(completion.status).toBe(label === "an SDK abort" ? "cancelled" : "failed");
    expect(completion.error).toEqual(expected);
    expect(JSON.stringify(completion)).not.toContain("test-key");
  });

  it("truncates a large HTTP error body", async () => {
    const error = new UnprocessableEntityError(422, { detail: "x".repeat(5_000) }, new Headers());
    const completion = await execute({
      createClient: fakeClient(() => {
        throw error;
      }).createClient,
    });
    const details = completion.error?.details as {
      status: number;
      retryable: boolean;
      body: string;
    };
    expect([details.status, details.retryable, details.body.length, details.body.at(-1)]).toEqual([
      422,
      false,
      2_001,
      "…",
    ]);
  });
});

describe("assessment response normalization", () => {
  const respondWith = (body: unknown) =>
    execute({ createClient: fakeClient(() => body).createClient });
  const answers = apiResponse.answers;

  it.each<[string, unknown, Record<string, unknown>]>([
    ["no usage", { model: "jev-1.13.0", answers }, {}],
    ["null usage", { model: "jev-1.13.0", answers, usage: null }, {}],
    [
      "one reported count",
      { model: "jev-1.13.0", answers, usage: { input_tokens: 10 } },
      { inputTokens: 10 },
    ],
    [
      "a null count",
      { model: "jev-1.13.0", answers, usage: { input_tokens: null, output_tokens: 3 } },
      { outputTokens: 3 },
    ],
  ])("keeps only the token counts the API reported: %s", async (_label, body, usage) => {
    const completion = await respondWith(body);
    expect(completion.status).toBe("succeeded");
    expect((completion.output as { usage: unknown }).usage).toEqual(usage);
    const decoded = await reviewDiff.decode(completion.output, operation);
    const input: number | undefined = decoded.usage.inputTokens;
    expect(input).toBe(usage.inputTokens);
  });

  it("tolerates floating-point noise and keeps the reported values", async () => {
    const completion = await respondWith(
      withAnswers({
        completeness: { ...answers.completeness, score: 2.0000000000000004 },
        risk: { ...answers.risk, confidence: 1.0000000000000002 },
        touchesTests: { type: "noul", noul: -1e-12 },
      }),
    );
    expect(completion.status).toBe("succeeded");
    const output = completion.output as { answers: { completeness: { score: number } } };
    expect(output.answers.completeness.score).toBe(2.0000000000000004);
  });

  it.each<[string, unknown, RegExp]>([
    [
      "an answer outside the options",
      withAnswers({ risk: { ...answers.risk, choice: "severe" } }),
      /answer 'risk' choice must be one of/,
    ],
    [
      "a missing answer",
      { ...apiResponse, answers: { risk: answers.risk } },
      /answers must contain exactly/,
    ],
    [
      "a fractional token count",
      { ...apiResponse, usage: { input_tokens: 1.5 } },
      /usage must be \{ inputTokens\?, outputTokens\? \} token counts/,
    ],
    [
      "a probability above 1",
      withAnswers({ touchesTests: { type: "noul", noul: 1.5 } }),
      /noul must be a probability/,
    ],
    [
      "a score beyond its tolerance",
      withAnswers({ completeness: { ...answers.completeness, score: 2.001 } }),
      /score must be between 0 and 2/,
    ],
    ["a blank model", { ...apiResponse, model: "" }, /model must be a non-empty string/],
    ["no body", null, /output must be \{ model, answers, usage \}/],
    ["a text body", "<html>", /output must be \{ model, answers, usage \}/],
    [
      "prototype keys",
      JSON.parse(
        '{"model":"m","answers":{"__proto__":{"type":"noul","noul":1},"touchesTests":{"type":"constructor"}},"usage":{}}',
      ),
      /answers must contain exactly the questions/,
    ],
  ])("fails a response with %s", async (_label, body, message) => {
    const completion = await respondWith(body);
    expect(completion.error).toEqual({
      message: expect.stringMatching(message),
      details: { phase: "response" },
    });
    expect(({} as Record<string, unknown>).noul).toBeUndefined();
  });

  it("fails a successful non-JSON response through the real SDK", async () => {
    const { createClient } = sdkClient(() => new Response("<html>"));
    expect((await execute({ createClient })).error?.details).toEqual({ phase: "response" });
  });
});

describe("assessment execution plumbing", () => {
  const Findings = guard("findings", (v): v is string[] => Array.isArray(v));
  const inspect = shell({ name: "inspect", command: "true", output: Findings });
  const triageQuestions = {
    action: {
      type: "choice" as const,
      instructions: "What should happen with these findings?",
      criteria: { autofix: null, review: null },
    },
  };
  const triage = assessment({
    name: "triage",
    input: Findings,
    ask: (findings) => ({ state: { findings }, questions: triageQuestions }),
  });

  it("runs through BridgeExecutor without spawning a bridge, records history, and replays", async () => {
    const { calls, createClient } = fakeClient(() => ({
      model: "jev-1.13.0",
      answers: {
        action: {
          type: "choice",
          choice: "autofix",
          probabilities: { autofix: 0.93, review: 0.07 },
          confidence: 0.9,
        },
      },
      usage: { input_tokens: 20, output_tokens: 3 },
    }));
    const events = new CollectingSink();
    const executor = new BridgeExecutor({
      bin: "/nonexistent/butterflow-execution-bridge",
      events,
      assessment: { apiKey: "test-key", createClient },
    });
    // Workflow code owns routing: the assessment only reports probabilities.
    const workflow = dynamic(async () => {
      const result = await triage({ input: ["unused import"] });
      const { action } = result.answers;
      return action.choice === "autofix" && action.confidence >= 0.8 ? "autofix" : "review";
    });
    const store = new MemoryHistoryStore();
    const first = await run(workflow, { executor, history: store });
    expect(first.output).toBe("autofix");
    expect(calls).toHaveLength(1);
    expect(calls[0]!.request.state).toEqual({ findings: ["unused import"] });
    expect(events.events.filter((e) => e.type === "bridge.spawned")).toHaveLength(0);
    const recorded = store.toJSON().events;
    expect(recorded.map((e) => e.type)).toEqual(["scheduled", "completed", "finalized"]);
    expect(recorded[0]).toMatchObject({
      command: { id: "triage", kind: "assessment", operation: { kind: "assessment" } },
    });

    const replay = await run(workflow, { executor, history: store });
    expect(replay.output).toBe("autofix");
    expect(calls).toHaveLength(1);
  });

  it("surfaces a failed assessment as an OperationError", async () => {
    const { createClient } = fakeClient(() => {
      throw new RateLimitError(429, undefined, new Headers());
    });
    const executor = new BridgeExecutor({
      bin: "/nonexistent/butterflow-execution-bridge",
      assessment: { apiKey: "test-key", createClient },
    });
    const error = await run(
      dynamic(() => triage({ input: [] })),
      { executor },
    ).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(OperationError);
    expect((error as OperationError).status).toBe("failed");
    expect((error as OperationError).detail).toMatchObject({
      details: { phase: "http", status: 429 },
    });
  });

  it("composes statically and is scriptable in the harness", async () => {
    const h = createHarness({
      results: {
        inspect: ["a", "b"],
        triage: {
          model: "jev-1.13.0",
          answers: {
            action: {
              type: "choice",
              choice: "review",
              probabilities: { autofix: 0.4, review: 0.6 },
              confidence: 0.3,
            },
          },
          usage: { inputTokens: 1, outputTokens: 1 },
        },
      },
    });
    const result = await h.run(sequence(inspect(), triage()));
    expect(result.output.answers.action.choice).toBe("review");
    expect(h.executed.map((request) => request.operation.kind)).toEqual(["shell", "assessment"]);
    expect(h.executed[1]!.operation).toEqual({
      kind: "assessment",
      state: { findings: ["a", "b"] },
      questions: triageQuestions,
    });
  });
});

describe("dynamic questions derived from input", () => {
  interface ClassifyInput {
    text: string;
    categories: string[];
  }
  const ClassifyInput = guard(
    "ClassifyInput",
    (v): v is ClassifyInput =>
      typeof v === "object" &&
      v !== null &&
      "text" in v &&
      typeof v.text === "string" &&
      "categories" in v &&
      Array.isArray(v.categories),
  );

  const classify = assessment({
    name: "classify",
    input: ClassifyInput,
    ask: (input) => ({
      state: { text: input.text },
      questions: {
        category: {
          type: "choice" as const,
          instructions: "Which category does the text belong to?",
          criteria: Object.fromEntries(input.categories.map((cat) => [cat, null])) as Record<
            string,
            null
          >,
        },
        confidence: {
          type: "noul" as const,
          instructions: "Is the classification confident?",
        },
      },
    }),
  });

  it("resolves questions from input and puts them on the operation", () => {
    const input: ClassifyInput = {
      text: "Hello",
      categories: ["greeting", "farewell", "question"],
    };
    const op = classify.toOperation(input);
    expect(op.kind).toBe("assessment");
    expect(op.state).toEqual({ text: "Hello" });
    expect(Object.keys(op.questions.category!.criteria as object)).toEqual([
      "greeting",
      "farewell",
      "question",
    ]);
    expect(op.questions.confidence!.type).toBe("noul");
  });

  it("produces different operations for different inputs", () => {
    const op1 = classify.toOperation({
      text: "A",
      categories: ["x", "y"],
    });
    const op2 = classify.toOperation({
      text: "B",
      categories: ["p", "q", "r"],
    });
    expect(Object.keys((op1.questions.category! as { criteria: object }).criteria)).toEqual([
      "x",
      "y",
    ]);
    expect(Object.keys((op2.questions.category! as { criteria: object }).criteria)).toEqual([
      "p",
      "q",
      "r",
    ]);
    expect(op1.state).toEqual({ text: "A" });
    expect(op2.state).toEqual({ text: "B" });
  });

  it("validates dynamic output against resolved questions", async () => {
    const input: ClassifyInput = { text: "Hi", categories: ["a", "b"] };
    const op = classify.toOperation(input);
    const validOutput = {
      model: "jev-1.13.0",
      answers: {
        category: {
          type: "choice",
          choice: "a",
          probabilities: { a: 0.8, b: 0.2 },
          confidence: 0.9,
        },
        confidence: { type: "noul", noul: 0.95 },
      },
      usage: { inputTokens: 10, outputTokens: 5 },
    };
    const result = await classify.decode(validOutput as Json, op);
    expect(result.answers.category.choice).toBe("a");
    expect(result.answers.confidence.noul).toBe(0.95);
  });

  it("rejects output with options not matching resolved questions", async () => {
    const input: ClassifyInput = { text: "Hi", categories: ["a", "b"] };
    const op = classify.toOperation(input);
    const badOutput = {
      model: "jev-1.13.0",
      answers: {
        category: {
          type: "choice",
          choice: "c",
          probabilities: { a: 0.4, b: 0.3, c: 0.3 },
          confidence: 0.5,
        },
        confidence: { type: "noul", noul: 0.5 },
      },
      usage: {},
    };
    await expect(classify.decode(badOutput as Json, op)).rejects.toThrow(/choice must be one of/);
  });

  it("rejects ask results with too few choice options", () => {
    const badClassify = assessment({
      name: "bad-classify",
      input: ClassifyInput,
      ask: (input) => ({
        state: { text: input.text },
        questions: {
          only: {
            type: "choice" as const,
            instructions: "Pick one",
            criteria: Object.fromEntries(
              input.categories.slice(0, 1).map((c) => [c, null]),
            ) as Record<string, null>,
          },
        },
      }),
    });
    expect(() => badClassify.toOperation({ text: "x", categories: ["only-one"] })).toThrow(
      "choice criteria must define at least two options",
    );
  });

  it("runs a dynamic-question workflow through the harness and replays", async () => {
    const input: ClassifyInput = { text: "Hello", categories: ["greeting", "farewell"] };
    const apiResult = {
      model: "jev-1.13.0",
      answers: {
        category: {
          type: "choice",
          choice: "greeting",
          probabilities: { greeting: 0.95, farewell: 0.05 },
          confidence: 0.92,
        },
        confidence: { type: "noul", noul: 0.88 },
      },
      usage: { inputTokens: 15, outputTokens: 4 },
    };
    const h = createHarness({ results: { classify: apiResult } });
    const workflow = dynamic(() => classify({ input }));
    const first = await h.run(workflow);
    expect(first.output.answers.category.choice).toBe("greeting");
    expect(h.executed).toHaveLength(1);
    const sentOp = h.executed[0]!.operation as AssessmentOperation;
    expect(Object.keys(sentOp.questions.category!.criteria as object)).toEqual([
      "greeting",
      "farewell",
    ]);

    // Replay from recorded history — same input yields same resolved questions.
    const replay = h.reload();
    const second = await replay.run(workflow);
    expect(second.output.answers.category.choice).toBe("greeting");
    expect(second.replayed).toBe(true);
    expect(replay.executed).toHaveLength(0);
  });
});

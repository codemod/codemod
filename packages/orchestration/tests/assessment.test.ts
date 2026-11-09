/**
 * Specs for `assessment()` and the decision-provider adapters. No test reaches
 * the network: requests go to a scripted `fetch`. The live check per provider
 * is `assessment.live.test.ts`.
 */
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createHarness } from "../src/host/harness.ts";
import {
  MemoryHistoryStore,
  NondeterminismError,
  assessment,
  canonicalJson,
  dynamic,
  executeAssessment,
  executeAssessmentOperation,
  getHost,
  guard,
  isOperationCompletion,
  isOperationRequest,
  parallel,
  run,
  sequence,
  type AssessmentExecutorOptions,
  type AssessmentOperation,
  type AssessmentQuestions,
  type AssessmentState,
  type DecisionFetch,
  type Json,
  type OperationRequest,
} from "../src/index.ts";

// --- helpers ---------------------------------------------------------------

const dirs: string[] = [];
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function repo(files: Record<string, string | Buffer>): string {
  const dir = mkdtempSync(join(tmpdir(), "assessment-"));
  dirs.push(dir);
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), content);
  }
  return dir;
}

interface Sent {
  url: string;
  headers: Record<string, string>;
  body: { [key: string]: Json };
}
type Reply = { status?: number; headers?: Record<string, string>; body: unknown } | Error;

/** A scripted decision API: `respond` sees each request and returns its reply. */
function api(respond: (sent: Sent, index: number) => Reply | Promise<Reply>) {
  const sent: Sent[] = [];
  const fetch: DecisionFetch = async (url, init) => {
    const request = { url, headers: init.headers, body: JSON.parse(init.body) };
    sent.push(request);
    const reply = await respond(request, sent.length - 1);
    if (reply instanceof Error) throw reply;
    const headers = new Map(Object.entries(reply.headers ?? {}));
    const text = typeof reply.body === "string" ? reply.body : JSON.stringify(reply.body);
    return {
      status: reply.status ?? 200,
      headers: { get: (n) => headers.get(n) ?? null },
      text: async () => text,
    };
  };
  return { sent, fetch };
}

const reviewQuestions = {
  risk: {
    type: "choice" as const,
    instructions: "How risky is this file?",
    criteria: { low: "Mechanical", medium: null, high: "Could change behavior" },
  },
  hasTests: { type: "yesno" as const, instructions: "Does this file contain tests?" },
} satisfies AssessmentQuestions;

/** A System One reply to `reviewQuestions`. */
const systemOneReply = (choice = "low") => ({
  body: {
    model: "jev-1.13.0",
    answers: {
      risk: {
        type: "choice",
        choice,
        probabilities: { low: 0.86, medium: 0.1, high: 0.04 },
        confidence: 0.81,
      },
      hasTests: { type: "noul", noul: 0.03 },
    },
    usage: { input_tokens: 312, output_tokens: 48 },
  },
});

/** The recorded result for `systemOneReply`. */
const reviewResult = (choice = "low") => ({
  provider: "systemone",
  model: "jev-1.13.0",
  calibrated: true,
  answers: {
    risk: {
      type: "choice",
      choice,
      probabilities: { low: 0.86, medium: 0.1, high: 0.04 },
      confidence: 0.81,
    },
    hasTests: { type: "yesno", yes: 0.03 },
  },
  usage: { inputTokens: 312, outputTokens: 48 },
});

const review = assessment({ name: "review", include: ["**/*.ts"], ask: () => reviewQuestions });

const batch = (...files: string[]) =>
  files.map((file) => ({ file, questions: reviewQuestions, assessment: reviewResult() })) as Json;

const KEY = { DECISION_API_KEY: "test-key" };

/** What a runtime's executor does with `assessment` operations. */
function executor(cwd: string, options: AssessmentExecutorOptions) {
  return {
    execute: (request: OperationRequest, signal?: AbortSignal) =>
      executeAssessmentOperation(
        { env: KEY, retry: { initialDelayMs: 0 }, ...options },
        cwd,
        request,
        signal,
      ),
  };
}

const files = async (dir: string, step = review, options: AssessmentExecutorOptions = {}) => {
  const { fetch } = api(() => systemOneReply());
  const { output } = await run(step, { executor: executor(dir, { fetch, ...options }) });
  return output.map((result) => result.file);
};

/** One single-file operation, as the batch executor builds per file. */
const single = (questions: AssessmentQuestions, provider: "systemone" | "openai" = "systemone") =>
  ({
    kind: "assessment",
    state: { file: { path: "a.ts", content: "code" } },
    questions,
    provider,
    model: provider === "systemone" ? "jev-latest" : "gpt-6-luna",
  }) satisfies AssessmentOperation;

const ask = (
  operation: AssessmentOperation,
  fetch: DecisionFetch,
  options: AssessmentExecutorOptions = {},
  signal?: AbortSignal,
) =>
  executeAssessment(
    { env: KEY, fetch, retry: { initialDelayMs: 0 }, ...options },
    "review",
    operation,
    signal,
  );

// --- defining an assessment ------------------------------------------------

describe("defining an assessment", () => {
  it.each<[string, object, string]>([
    ["a blank name", { name: " " }, "assessment name must not be empty"],
    ["ask that is not a function", { ask: "no" }, "ask must be a function"],
    ["a blank pinned model", { model: "" }, "model must not be empty"],
    ["no include patterns", { include: [] }, "include must be a non-empty list"],
    ["a blank include pattern", { include: [""] }, "include patterns must be non-empty strings"],
    ["an empty exclude list", { exclude: [] }, "exclude must be a non-empty list"],
    ["a blank exclude pattern", { exclude: [""] }, "exclude patterns must be non-empty strings"],
  ])("rejects %s", (_, override, message) => {
    const options = { name: "a", include: ["**/*.ts"], ask: () => reviewQuestions, ...override };
    expect(() => assessment(options as never)).toThrow(message);
  });

  const recorded = [{ file: "a.ts", assessment: reviewResult() }];

  it.each<[string, () => unknown, unknown]>([
    ["the root", () => review, recorded],
    ["a step in sequence()", () => sequence(review()), recorded],
    ["a branch of parallel()", () => parallel(review()), [recorded]],
    ["awaited inside dynamic()", () => dynamic(async () => await review()), recorded],
  ])("runs as %s", async (_, root, expected) => {
    const h = createHarness({ results: { review: batch("a.ts") } });
    const { output } = await h.run(root() as typeof review);
    expect(output).toEqual(expected);
  });
});

// --- the recorded operation ------------------------------------------------

describe("the recorded operation", () => {
  const Input = guard("Input", (v): v is { threshold: number } => typeof v === "object");
  const withInput = assessment({
    name: "review",
    include: ["src/**"],
    exclude: ["**/*.test.ts"],
    input: Input,
    ask: () => reviewQuestions,
  });

  it("holds the selection, the input, the provider and the model, and nothing of ask", () => {
    expect(JSON.parse(canonicalJson(withInput.toOperation({ threshold: 0.5 })))).toEqual({
      kind: "assessment",
      state: { include: ["src/**"], exclude: ["**/*.test.ts"], input: { threshold: 0.5 } },
      questions: { __batch: { type: "yesno", instructions: "file-oriented batch assessment" } },
      provider: "systemone",
      model: "jev-latest",
    });
  });

  it.each<[string, Record<string, string>, string | undefined, object]>([
    [
      "DECISION_PROVIDER and its default model",
      { DECISION_PROVIDER: "openai" },
      undefined,
      { provider: "openai", model: "gpt-6-luna" },
    ],
    ["DECISION_MODEL", { DECISION_MODEL: "jev-1.13.0" }, undefined, { model: "jev-1.13.0" }],
    [
      "TYPESAFE_DEFAULT_MODEL for System One",
      { TYPESAFE_DEFAULT_MODEL: "jev-2" },
      undefined,
      { model: "jev-2" },
    ],
    [
      "a pinned model over DECISION_MODEL",
      { DECISION_MODEL: "jev-1.13.0" },
      "jev-pinned",
      { model: "jev-pinned" },
    ],
  ])("takes %s", (_, env, model, expected) => {
    for (const [name, value] of Object.entries(env)) vi.stubEnv(name, value);
    const step = assessment({ name: "r", include: ["*"], ask: () => reviewQuestions, model });
    expect(step.toOperation(undefined)).toMatchObject(expected);
  });

  it("rejects an unknown DECISION_PROVIDER", () => {
    vi.stubEnv("DECISION_PROVIDER", "other");
    expect(() => review.toOperation(undefined)).toThrow("DECISION_PROVIDER 'other' is not one of");
  });

  it("replays as changed when the configured model changes", async () => {
    const h = createHarness({ results: { review: batch("a.ts") } });
    await h.run(review);
    vi.stubEnv("DECISION_MODEL", "jev-2");
    await expect(h.reload().run(review)).rejects.toThrow(NondeterminismError);
  });
});

// --- decoding a recorded result --------------------------------------------

describe("decoding a recorded result", () => {
  const operation = review.toOperation(undefined);

  it.each([null, undefined, []])("reads %j as no files", async (output) => {
    expect(await review.decode(output as Json, operation)).toEqual([]);
  });

  it.each<[string, unknown, string | RegExp]>([
    ["a non-array output", "bad", "expected an array"],
    [
      "an entry without a file",
      [{ questions: reviewQuestions, assessment: reviewResult() }],
      "string 'file' field",
    ],
    [
      "an entry with invalid questions",
      [{ file: "a.ts", questions: {}, assessment: reviewResult() }],
      "invalid questions",
    ],
    [
      "answers that miss a question",
      [
        {
          file: "a.ts",
          questions: reviewQuestions,
          assessment: { ...reviewResult(), answers: {} },
        },
      ],
      /answers must contain exactly/,
    ],
    [
      "a result without the provider",
      [
        {
          file: "a.ts",
          questions: reviewQuestions,
          assessment: { ...reviewResult(), provider: undefined },
        },
      ],
      "provider must be a non-empty string",
    ],
  ])("rejects %s", async (_, output, message) => {
    await expect(review.decode(output as Json, operation)).rejects.toThrow(message);
  });

  it("accepts a refusal in place of any answer", async () => {
    const refused = {
      ...reviewResult(),
      answers: { ...reviewResult().answers, risk: { type: "refused" } },
    };
    const output = [{ file: "a.ts", questions: reviewQuestions, assessment: refused }] as Json;
    expect(await review.decode(output, operation)).toEqual([{ file: "a.ts", assessment: refused }]);
  });
});

// --- selecting and reading files -------------------------------------------

describe("selecting and reading files", () => {
  it("selects by include and exclude from the target root, in walker order", async () => {
    const dir = repo({
      "z.ts": "",
      "m/b.ts": "",
      "a.ts": "",
      "a.test.ts": "",
      "test/e.ts": "",
      "r.md": "",
    });
    const step = assessment({
      name: "review",
      include: ["**/*.ts"],
      exclude: ["test/**", "**/*.test.ts"],
      ask: () => reviewQuestions,
    });
    expect(await files(dir, step)).toEqual(["a.ts", "m/b.ts", "z.ts"]);
  });

  it("skips symlinks", async () => {
    const dir = repo({ "real.ts": "" });
    symlinkSync(join(dir, "real.ts"), join(dir, "link.ts"));
    expect(await files(dir)).toEqual(["real.ts"]);
  });

  it("asks nothing and returns no results when no file matches", async () => {
    const { sent, fetch } = api(() => systemOneReply());
    await run(review, { executor: executor(repo({ "r.md": "" }), { fetch }) });
    expect(sent).toEqual([]);
  });

  it("sends each file's path and content, and the workflow input, as the state", async () => {
    const step = assessment({
      name: "review",
      include: ["*.ts"],
      input: guard("I", (v): v is { m: string } => typeof v === "object"),
      ask: () => reviewQuestions,
    });
    const { sent, fetch } = api(() => systemOneReply());
    await run(step, {
      executor: executor(repo({ "a.ts": "code" }), { fetch }),
      input: { m: "v2" },
    });
    expect(sent[0]!.body.state).toEqual({
      file: { path: "a.ts", content: "code" },
      input: { m: "v2" },
    });
  });

  it("asks the questions ask returns for each file", async () => {
    const step = assessment({
      name: "review",
      include: ["*"],
      ask: ({ file }) => ({
        kind: { type: "yesno" as const, instructions: `is ${file.path} a test?` },
      }),
    });
    const { sent, fetch } = api(() => ({
      body: { model: "m", answers: { kind: { type: "noul", noul: 0.5 } } },
    }));
    await run(step, { executor: executor(repo({ "a.ts": "", "b.ts": "" }), { fetch }) });
    expect(
      sent.map((s) => (s.body.questions as { kind: { instructions: string } }).kind.instructions),
    ).toEqual(["is a.ts a test?", "is b.ts a test?"]);
  });

  it("fails, naming the file, when ask returns invalid questions", async () => {
    const step = assessment({ name: "review", include: ["*"], ask: () => ({}) });
    const { fetch } = api(() => systemOneReply());
    await expect(
      run(step, { executor: executor(repo({ "a.ts": "" }), { fetch }) }),
    ).rejects.toThrow("questions for 'a.ts': questions must contain at least one question");
  });

  it("fails, naming the file, when a file is not valid UTF-8", async () => {
    const dir = repo({ "good.ts": "", "bad.ts": Buffer.from([0x80, 0x81]) });
    await expect(files(dir)).rejects.toThrow("file 'bad.ts' is not valid UTF-8");
  });

  it("fails, naming the file, when a file cannot be read", async () => {
    const dir = repo({ "a.ts": "" });
    vi.spyOn(getHost().fs, "readText").mockImplementation(() => {
      throw Object.assign(new Error("permission denied"), { code: "EACCES" });
    });
    await expect(files(dir)).rejects.toThrow("reading 'a.ts' failed: permission denied");
  });

  it("skips a file that vanished after selection", async () => {
    const dir = repo({ "a.ts": "", "b.ts": "" });
    const readText = getHost().fs.readText;
    vi.spyOn(getHost().fs, "readText").mockImplementation((file, options) => {
      if (file.endsWith("a.ts")) throw Object.assign(new Error("gone"), { code: "ENOENT" });
      return readText(file, options);
    });
    expect(await files(dir)).toEqual(["b.ts"]);
  });
});

// --- replay ----------------------------------------------------------------

describe("replay", () => {
  it("returns the recorded results without selecting, reading or asking", async () => {
    const dir = repo({ "a.ts": "" });
    const history = new MemoryHistoryStore();
    const first = await run(review, {
      executor: executor(
        dir,
        api(() => systemOneReply()),
      ),
      history,
    });
    rmSync(dir, { recursive: true });
    const never = api(() => new Error("replay must not ask"));
    const second = await run(review, { executor: executor(dir, never), history });
    expect(second.output).toEqual(first.output);
  });
});

// --- concurrency and failure -----------------------------------------------

describe("asking many files", () => {
  const many = (count: number) =>
    repo(
      Object.fromEntries(
        Array.from({ length: count }, (_, i) => [`f${String(i).padStart(2, "0")}.ts`, ""]),
      ),
    );

  async function peak(dir: string, options: AssessmentExecutorOptions = {}) {
    let active = 0;
    let max = 0;
    const { fetch } = api(async () => {
      max = Math.max(max, ++active);
      await new Promise((r) => setTimeout(r, 10));
      active--;
      return systemOneReply();
    });
    await run(review, { executor: executor(dir, { fetch, ...options }) });
    return max;
  }

  it("asks at most fileConcurrency files at once", async () => {
    expect(await peak(many(8), { fileConcurrency: 2 })).toBe(2);
  });

  it("asks at most 4 files at once by default", async () => {
    expect(await peak(many(12))).toBe(4);
  });

  it("returns results in selector order whatever order the answers arrive in", async () => {
    const { fetch } = api(async (_, index) => {
      if (index % 2 === 0) await new Promise((r) => setTimeout(r, 20));
      return systemOneReply();
    });
    const { output } = await run(review, {
      executor: executor(many(4), { fetch, fileConcurrency: 2 }),
    });
    expect(output.map((r) => r.file)).toEqual(["f00.ts", "f01.ts", "f02.ts", "f03.ts"]);
  });

  it("fails the step with the first file's failure", async () => {
    const { fetch } = api((_, index) =>
      index === 0 ? new Error("network down") : systemOneReply(),
    );
    await expect(
      run(review, { executor: executor(many(3), { fetch, retry: { maxRetries: 0 } }) }),
    ).rejects.toThrow("decision request failed: network down");
  });

  it("starts no further file after a failure", async () => {
    const { sent, fetch } = api(() => ({ status: 400, body: {} }));
    await run(review, { executor: executor(many(6), { fetch, fileConcurrency: 1 }) }).catch(
      () => {},
    );
    expect(sent).toHaveLength(1);
  });

  it("stops when the run is aborted", async () => {
    const controller = new AbortController();
    const { fetch } = api((_, index) => {
      if (index === 1) controller.abort();
      return systemOneReply();
    });
    const running = run(review, {
      executor: executor(many(5), { fetch, fileConcurrency: 1 }),
      signal: controller.signal,
    });
    await expect(running).rejects.toThrow();
  });
});

// --- the System One adapter ------------------------------------------------

describe("the System One adapter", () => {
  const yesNo = {
    ok: { type: "yesno" as const, instructions: "ok?", criteria: { yes: "fine", no: "broken" } },
  };
  const yesNoReply = { body: { model: "jev-1", answers: { ok: { type: "noul", noul: 0.9 } } } };

  it("posts state, questions and model to /v1/systemone with a bearer key", async () => {
    const { sent, fetch } = api(() => systemOneReply());
    await ask(single(reviewQuestions), fetch);
    expect(sent[0]).toEqual({
      url: "https://api.typesafe.ai/v1/systemone",
      headers: {
        authorization: "Bearer test-key",
        accept: "application/json",
        "content-type": "application/json",
      },
      body: {
        state: { file: { path: "a.ts", content: "code" } },
        questions: {
          ...reviewQuestions,
          hasTests: { type: "noul", instructions: "Does this file contain tests?" },
        },
        model: "jev-latest",
      },
    });
  });

  it("sends a yes/no question as noul with true and false criteria", async () => {
    const { sent, fetch } = api(() => yesNoReply);
    await ask(single(yesNo), fetch);
    expect(sent[0]!.body.questions).toEqual({
      ok: { type: "noul", instructions: "ok?", criteria: { true: "fine", false: "broken" } },
    });
  });

  it("returns the answers in this package's shapes, with provider, model and calibration", async () => {
    const { fetch } = api(() => systemOneReply());
    expect(await ask(single(reviewQuestions), fetch)).toMatchObject({
      status: "succeeded",
      output: reviewResult(),
    });
  });

  it("drops answer fields the API adds later", async () => {
    const { fetch } = api(() => ({
      body: { model: "m", answers: { ok: { type: "noul", noul: 0.9, extra: 1 } } },
    }));
    expect(await ask(single(yesNo), fetch)).toMatchObject({
      output: { answers: { ok: { type: "yesno", yes: 0.9 } } },
    });
  });

  it("reaches Microsoft Decision on Foundry with a configured path and api-key header", async () => {
    const env = {
      DECISION_BASE_URL: "https://x.services.ai.azure.com/",
      DECISION_PATH: "providers/microsoft/v1/systemone",
      DECISION_AUTH_HEADER: "api-key",
      DECISION_API_KEY: "azure",
    };
    const { sent, fetch } = api(() => yesNoReply);
    await ask(single(yesNo), fetch, { env });
    expect([sent[0]!.url, sent[0]!.headers["api-key"], sent[0]!.headers.authorization]).toEqual([
      "https://x.services.ai.azure.com/providers/microsoft/v1/systemone",
      "azure",
      undefined,
    ]);
  });

  it("falls back to TYPESAFE_API_KEY and TYPESAFE_BASE_URL", async () => {
    const env = { TYPESAFE_API_KEY: "legacy", TYPESAFE_BASE_URL: "http://127.0.0.1:9" };
    const { sent, fetch } = api(() => yesNoReply);
    await ask(single(yesNo), fetch, { env });
    expect([sent[0]!.url, sent[0]!.headers.authorization]).toEqual([
      "http://127.0.0.1:9/v1/systemone",
      "Bearer legacy",
    ]);
  });
});

// --- the OpenAI adapter ----------------------------------------------------

describe("the OpenAI decisions adapter", () => {
  const questions = {
    ok: { type: "yesno" as const, instructions: "ok?", criteria: { yes: "fine" } },
    risk: {
      type: "choice" as const,
      instructions: { ask: "risk?" },
      criteria: { low: null, high: "breaks" },
    },
    size: { type: "score" as const, instructions: "size?", criteria: ["small", { big: true }] },
  } satisfies AssessmentQuestions;
  const reply = {
    body: {
      model: "gpt-6-luna-2026",
      answers: [
        { type: "predicate", name: "ok", probability: 0.7 },
        {
          type: "choice",
          name: "risk",
          choice: "high",
          probabilities: [
            { value: "low", probability: 0.2 },
            { value: "high", probability: 0.8 },
          ],
          confidence: 0.6,
        },
        {
          type: "score",
          name: "size",
          score: 0.4,
          probabilities: [
            { value: 0, label: "0", probability: 0.6 },
            { value: 1, label: "1", probability: 0.4 },
          ],
          confidence: 0.5,
        },
      ],
      usage: { input_tokens: 40, input_tokens_details: { cached_tokens: 0 } },
    },
  };
  const env = { DECISION_PROVIDER: "openai", OPENAI_API_KEY: "sk-test" };

  it("posts the questions as a named array to /v1/decisions with OPENAI_API_KEY", async () => {
    const { sent, fetch } = api(() => reply);
    await ask(single(questions, "openai"), fetch, { env });
    expect(sent[0]).toMatchObject({
      url: "https://api.openai.com/v1/decisions",
      headers: { authorization: "Bearer sk-test" },
      body: {
        model: "gpt-6-luna",
        questions: [
          { type: "predicate", name: "ok", instructions: "ok?\n\nAnswer yes when: fine" },
          {
            type: "choice",
            name: "risk",
            instructions: '{"ask":"risk?"}',
            choices: [
              { value: "low", description: "low" },
              { value: "high", description: "breaks" },
            ],
          },
          {
            type: "score",
            name: "size",
            instructions: "size?",
            levels: [
              { label: "0", description: "small" },
              { label: "1", description: '{"big":true}' },
            ],
          },
        ],
      },
    });
  });

  it.each<[string, AssessmentState, unknown]>([
    ["text state as the input string", "a diff", "a diff"],
    [
      "JSON state as an input_json message",
      { a: 1 },
      [{ role: "user", content: [{ type: "input_json", data: { a: 1 } }] }],
    ],
  ])("sends %s", async (_, state, input) => {
    const { sent, fetch } = api(() => reply);
    await ask({ ...single(questions, "openai"), state }, fetch, { env });
    expect(sent[0]!.body.input).toEqual(input);
  });

  it("maps the answers back by name, with the legend from the question", async () => {
    const { fetch } = api(() => reply);
    expect((await ask(single(questions, "openai"), fetch, { env })).output).toEqual({
      provider: "openai",
      model: "gpt-6-luna-2026",
      calibrated: true,
      answers: {
        ok: { type: "yesno", yes: 0.7 },
        risk: {
          type: "choice",
          choice: "high",
          probabilities: { low: 0.2, high: 0.8 },
          confidence: 0.6,
        },
        size: {
          type: "score",
          score: 0.4,
          legend: { 0: "small", 1: { big: true } },
          probabilities: { 0: 0.6, 1: 0.4 },
          confidence: 0.5,
        },
      },
      usage: { inputTokens: 40 },
    });
  });

  it("returns a refusal as a refused answer", async () => {
    const { fetch } = api(() => ({ body: { answers: [{ type: "refusal", name: "ok" }] } }));
    expect(
      (await ask(single({ ok: questions.ok }, "openai"), fetch, { env })).output,
    ).toMatchObject({ answers: { ok: { type: "refused" } } });
  });

  it.each<[string, AssessmentQuestions, string]>([
    [
      "more than 10 score levels",
      { s: { type: "score", instructions: "s", criteria: Array(11).fill(null) } },
      "question 's': openai decisions take at most 10 score levels",
    ],
    [
      "more than 400 questions",
      Object.fromEntries(
        Array.from({ length: 401 }, (_, i) => [`q${i}`, { type: "yesno", instructions: "?" }]),
      ),
      "openai decisions take at most 400 questions per request",
    ],
  ])("refuses %s before sending", async (_, many, message) => {
    const { fetch } = api(() => reply);
    expect((await ask(single(many, "openai"), fetch, { env })).error).toEqual({
      message,
      details: { phase: "request" },
    });
  });

  it("fails when an answer repeats a question name", async () => {
    const { fetch } = api(() => ({
      body: { answers: [reply.body.answers[0], reply.body.answers[0]] },
    }));
    expect((await ask(single({ ok: questions.ok }, "openai"), fetch, { env })).error?.message).toBe(
      "decision API returned an invalid assessment: answers repeat 'ok'",
    );
  });
});

// --- configuration and transport failures ----------------------------------

describe("assessment failures", () => {
  const yesNo = { ok: { type: "yesno" as const, instructions: "ok?" } };
  const ok = { body: { model: "m", answers: { ok: { type: "noul", noul: 1 } } } };
  const failure = async (
    reply: () => Reply | Promise<Reply>,
    options: AssessmentExecutorOptions = {},
  ) =>
    (
      await ask(single(yesNo), api(reply).fetch, {
        retry: { maxRetries: 0, initialDelayMs: 0 },
        ...options,
      })
    ).error;

  it.each<[string, AssessmentExecutorOptions, string]>([
    ["no API key", { env: {} }, "no API key for the systemone decision API: set DECISION_API_KEY"],
    [
      "a base URL that is not http(s)",
      { env: { ...KEY, DECISION_BASE_URL: "ftp://x" } },
      "decision base URL 'ftp://x' is not http(s)",
    ],
    [
      "a base URL with a query",
      { env: { ...KEY, DECISION_BASE_URL: "https://x/?a=1" } },
      "decision base URL 'https://x/?a=1' must not carry a query, fragment, or user",
    ],
    [
      "a timeout too large for a timer",
      { timeoutMs: 2 ** 31 },
      "timeoutMs must be between 1 and 2147483647",
    ],
    [
      "more retries than the cap",
      { retry: { maxRetries: 11 } },
      "retry.maxRetries must be at most 10",
    ],
  ])("fails as config with %s", async (_, options, message) => {
    expect(await failure(() => ok, options)).toEqual({ message, details: { phase: "config" } });
  });

  it("fails an HTTP error with its status, request id and body", async () => {
    const reply = { status: 400, headers: { "x-request-id": "req-1" }, body: { error: "bad" } };
    expect(await failure(() => reply)).toEqual({
      message: "decision API responded 400",
      details: {
        phase: "http",
        status: 400,
        retryable: false,
        requestId: "req-1",
        body: { error: "bad" },
      },
    });
  });

  it("reports the server's retry-after-ms on a rate limit", async () => {
    const reply = { status: 429, headers: { "retry-after-ms": "1500" }, body: "" };
    expect((await failure(() => reply))?.details).toMatchObject({
      retryable: true,
      retryAfterMs: 1500,
    });
  });

  it.each<[string, Reply]>([
    ["429", { status: 429, body: "" }],
    ["503", { status: 503, body: "" }],
    ["a connection error", new Error("reset")],
  ])("retries %s up to maxRetries", async (_, reply) => {
    const { sent, fetch } = api(() => reply);
    await ask(single(yesNo), fetch, { retry: { maxRetries: 2, initialDelayMs: 0 } });
    expect(sent).toHaveLength(3);
  });

  it("succeeds when a retry is answered", async () => {
    const { fetch } = api((_, index) => (index === 0 ? { status: 500, body: "" } : ok));
    expect((await ask(single(yesNo), fetch)).status).toBe("succeeded");
  });

  it("fails a request that outlives timeoutMs as timed out", async () => {
    const hang = (_: string, init: { signal: AbortSignal }) =>
      new Promise<never>((_, reject) =>
        init.signal.addEventListener("abort", () => reject(new Error("aborted"))),
      );
    const completion = await ask(single(yesNo), hang as unknown as DecisionFetch, {
      timeoutMs: 5,
      retry: { maxRetries: 0 },
    });
    expect(completion.error).toEqual({
      message: "decision request timed out after 5ms",
      details: { phase: "request", timedOut: true, retryable: true },
    });
  });

  it("fails a connection error in the request phase", async () => {
    expect(await failure(() => new Error("ECONNREFUSED"))).toEqual({
      message: "decision request failed: ECONNREFUSED",
      details: { phase: "request", retryable: true },
    });
  });

  it("fails a 2xx body that is not JSON in the response phase", async () => {
    expect(await failure(() => ({ body: "<html>" }))).toEqual({
      message: "decision API returned a body that is not JSON",
      details: { phase: "response", status: 200 },
    });
  });

  it("fails answers that do not match the questions in the response phase", async () => {
    expect(await failure(() => ({ body: { model: "m", answers: {} } }))).toEqual({
      message:
        "decision API returned an invalid assessment: answers must contain exactly the questions 'ok'",
      details: { phase: "response" },
    });
  });

  it("is cancelled when aborted before it starts", async () => {
    expect((await ask(single(yesNo), api(() => ok).fetch, {}, AbortSignal.abort())).status).toBe(
      "cancelled",
    );
  });

  it("is cancelled when aborted during the request", async () => {
    const controller = new AbortController();
    const { fetch } = api(() => {
      controller.abort();
      return new Error("aborted");
    });
    expect((await ask(single(yesNo), fetch, {}, controller.signal)).status).toBe("cancelled");
  });
});

// --- protocol fixtures -----------------------------------------------------

describe("protocol fixtures", () => {
  const fixture = (name: string) =>
    JSON.parse(readFileSync(join(import.meta.dirname, "../fixtures/protocol", name), "utf8"));

  it("accept the assessment request", () => {
    expect(isOperationRequest(fixture("assessment-request.json"))).toBe(true);
  });

  it("accept the assessment completion", () => {
    expect(isOperationCompletion(fixture("assessment-completion.json"))).toBe(true);
  });
});

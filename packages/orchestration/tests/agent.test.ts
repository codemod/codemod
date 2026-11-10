/**
 * Mocked contract tests for `agent()`: request serialization, response
 * parsing, validation and failure behavior, the launch environment
 * (`agent-env.ts`), and process-tree cleanup. Execution through the
 * `codemod-agent` runner is `agent-runner.test.ts`. No model is called; the
 * live checks are `agent.*.live.test.ts`.
 */
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { descendants, killProcessTree } from "./support/process-tree.ts";
import { createHarness } from "./support/harness.ts";
import {
  DEFAULT_BUILTIN_AGENT_TOOLS,
  DEFAULT_CLAUDE_CODE_TOOLS,
  MemoryHistoryStore,
  NondeterminismError,
  OperationError,
  PROTOCOL_VERSION,
  SchemaError,
  agent,
  agentEnvironment,
  agentLaunch,
  externalAgentEnvProblem,
  isSecretEnvName,
  dynamic,
  executeAgent,
  guard,
  isOperation,
  run,
  sequence,
  shell,
  type OperationRequest,
} from "../src/index.ts";

const dir = join(import.meta.dirname, "..", "fixtures", "protocol");
const fixture = (name: string) => JSON.parse(readFileSync(join(dir, name), "utf8")) as unknown;
const fakeAgent = resolve(import.meta.dirname, "fixtures/fake-agent.mjs");

interface Packages {
  packages: string[];
}
const Packages = guard(
  "packages",
  (v): v is Packages => typeof v === "object" && v !== null && "packages" in v,
);
interface Fixed {
  fixed: string[];
}
const Fixed = guard(
  "fixed",
  (v): v is Fixed =>
    typeof v === "object" && v !== null && Array.isArray((v as { fixed?: unknown }).fixed),
);

const fixTests = agent({
  name: "fix-tests",
  prompt: "Fix the failing tests in the listed packages.",
  input: Packages,
});

type Backend = Parameters<typeof agent>[0]["backend"];

const builtin = (overrides: Record<string, unknown> = {}) => ({
  kind: "builtin",
  tools: [...DEFAULT_BUILTIN_AGENT_TOOLS],
  ...overrides,
});

const operation = (overrides: Record<string, unknown> = {}) => ({
  kind: "agent",
  prompt: "p",
  backend: builtin(),
  ...overrides,
});

describe("agent request serialization", () => {
  const Version = guard("version", (v): v is { version: string } => typeof v === "object");

  it.each([
    [
      "agent-request.json",
      agent({
        name: "fix-tests",
        prompt: "Fix the failing tests in the listed packages. Report the fixed files.",
        input: Packages,
        output: Fixed,
        backend: { kind: "builtin", maxSteps: 40 },
      }).toOperation({ packages: ["web", "api"] }),
    ],
    [
      "agent-claude-code-request.json",
      agent({
        name: "summarize-readme",
        prompt: "Summarize README.md in one sentence.",
        backend: { kind: "claude-code", tools: ["Read", "Glob", "Grep"] },
      }).toOperation(),
    ],
    [
      "agent-codex-request.json",
      agent({
        name: "bump-version",
        prompt: "Set the version in VERSION to 2.0.0.",
        input: Version,
        output: Version,
        backend: { kind: "codex" },
      }).toOperation({ version: "2.0.0" }),
    ],
  ])("builds the operation of the shared fixture %s", (name, op) => {
    expect(op).toEqual((fixture(name) as { operation: unknown }).operation);
  });

  it.each<[string, Backend, unknown?]>([
    ["no backend as the builtin default tools", undefined, builtin()],
    ["a builtin backend without tools as the default tools", { kind: "builtin" }, builtin()],
    [
      "builtin tools and maxSteps",
      { kind: "builtin", tools: ["bash", "glob"], maxSteps: 5 },
      builtin({ tools: ["bash", "glob"], maxSteps: 5 }),
    ],
    ["an empty builtin tool set", { kind: "builtin", tools: [] }, builtin({ tools: [] })],
    [
      "claude-code without tools as the default file tools",
      { kind: "claude-code" },
      { kind: "claude-code", tools: ["Read", "Edit", "Write", "Glob", "Grep"] },
    ],
    ["claude-code tools as given", { kind: "claude-code", tools: ["Read", "Bash"] }],
    [
      "codex without a sandbox as workspace-write",
      { kind: "codex" },
      { kind: "codex", sandbox: "workspace-write" },
    ],
    ["a read-only codex sandbox as given", { kind: "codex", sandbox: "read-only" }],
  ])("records %s", (_name, backend, recorded = backend) => {
    const options = backend === undefined ? {} : { backend };
    expect(agent({ name: "s", prompt: "p", ...options }).toOperation()).toEqual(
      operation({ backend: recorded }),
    );
  });

  it("leaves shell and unconfined tools out of every default tool set", () => {
    const defaults: string[] = [...DEFAULT_BUILTIN_AGENT_TOOLS, ...DEFAULT_CLAUDE_CODE_TOOLS];
    const unconfined = ["bash", "mcp_tool", "ckg_tool", "Bash"];
    expect(defaults.filter((tool) => unconfined.includes(tool))).toEqual([]);
  });

  const define = (options: Record<string, unknown>) => () =>
    agent({ name: "x", prompt: "p", ...options } as never);

  it.each<[string, string[], string]>([
    ["builtin", ["rm_rf"], "agent 'x': builtin tools must be distinct names from 'bash'"],
    ["builtin", ["glob", "glob"], "builtin tools must be distinct"],
    ["claude-code", ["bash"], "claude-code tools must be distinct names from 'Read'"],
    ["claude-code", ["Read", "Read"], "claude-code tools must be distinct"],
  ])("refuses %s tools %j when the step is defined", (kind, tools, message) => {
    expect(define({ backend: { kind, tools } })).toThrow(message);
  });

  it.each([0, 1.5])("refuses builtin maxSteps %s when the step is defined", (maxSteps) => {
    expect(define({ backend: { kind: "builtin", maxSteps } })).toThrow(
      "agent 'x': builtin maxSteps must be a positive integer",
    );
  });

  it.each<[string, string, unknown]>([
    ["builtin", "sandbox", "read-only"],
    ["claude-code", "maxSteps", 3],
    ["codex", "tools", ["Read"]],
    ["codex", "maxSteps", 3],
  ])("refuses a %s backend's unsupported '%s' when the step is defined", (kind, key, value) => {
    expect(define({ backend: { kind, [key]: value } })).toThrow(
      `agent 'x': backend '${kind}' does not support '${key}'`,
    );
  });

  it.each<[string, Record<string, unknown>, string]>([
    [
      "an unsafe codex sandbox",
      { backend: { kind: "codex", sandbox: "danger-full-access" } },
      "codex sandbox must be one of 'read-only', 'workspace-write'",
    ],
    [
      "an unknown backend kind",
      { backend: { kind: "opencode" } },
      "backend kind must be one of 'builtin', 'claude-code', 'codex'",
    ],
    ["a null backend", { backend: null }, "agent 'x': backend must be an object"],
    [
      "tools outside the backend",
      { tools: ["glob"] },
      "unknown option 'tools'; backend settings such as tools belong in 'backend'",
    ],
    ["maxSteps outside the backend", { maxSteps: 3 }, "unknown option 'maxSteps'"],
  ])("refuses %s when the step is defined", (_name, options, message) => {
    expect(define(options)).toThrow(message);
  });

  it.each<[string, unknown]>([
    [
      "a builtin step with input, maxSteps and a JSON response",
      operation({ input: { a: 1 }, backend: builtin({ maxSteps: 3 }), responseFormat: "json" }),
    ],
    [
      "a claude-code step with no tools",
      operation({ backend: { kind: "claude-code", tools: [] } }),
    ],
    ["a read-only codex step", operation({ backend: { kind: "codex", sandbox: "read-only" } })],
  ])("accepts %s on the wire", (_name, value) => {
    expect(isOperation(value)).toBe(true);
  });

  it.each<[string, unknown]>([
    ["no backend", { kind: "agent", prompt: "p" }],
    ["tools outside the backend", { kind: "agent", prompt: "p", tools: [] }],
    ["an unknown builtin tool", operation({ backend: builtin({ tools: ["shell"] }) })],
    ["a repeated builtin tool", operation({ backend: builtin({ tools: ["glob", "glob"] }) })],
    ["zero builtin maxSteps", operation({ backend: builtin({ maxSteps: 0 }) })],
    [
      "claude-code maxSteps",
      operation({ backend: { kind: "claude-code", tools: ["Read"], maxSteps: 2 } }),
    ],
    ["a codex backend without a sandbox", operation({ backend: { kind: "codex" } })],
    [
      "an unsafe codex sandbox",
      operation({ backend: { kind: "codex", sandbox: "danger-full-access" } }),
    ],
    ["an XML response format", operation({ responseFormat: "xml" })],
    ["non-JSON input", operation({ input: Number.NaN })],
    ["a model", operation({ model: "x" })],
    ["the removed 'ai' kind", { kind: "ai", prompt: "p" }],
  ])("rejects %s on the wire", (_name, value) => {
    expect(isOperation(value)).toBe(false);
  });
});

describe("agent response parsing", () => {
  const structured = agent({ name: "fix", prompt: "Report fixed files.", output: Fixed });
  const op = structured.toOperation();
  /** Agent decode ignores the operation; this one only satisfies the signature. */
  const dummyOp = fixTests.toOperation({ packages: [] });

  it("decodes the response as { text } when the step has no output schema", async () => {
    await expect(fixTests.decode({ text: "Fixed 2 tests." }, dummyOp)).resolves.toEqual({
      text: "Fixed 2 tests.",
    });
  });

  it.each([
    ["bare JSON", '{"fixed":["a.test.ts"]}'],
    ["JSON with surrounding whitespace", '  {"fixed":["a.test.ts"]}\n'],
    ["a single ```json block", '```json\n{"fixed":["a.test.ts"]}\n```'],
    [
      "a ```json block after prose",
      'Done. Here is the result:\n\n```json\n{\n  "fixed": ["a.test.ts"]\n}\n```\n',
    ],
    ["a single unlabelled fenced block", '```\n{"fixed":["a.test.ts"]}\n```'],
  ])("parses the output from %s", async (_name, text) => {
    await expect(structured.decode({ text }, op)).resolves.toEqual({ fixed: ["a.test.ts"] });
  });

  it.each([
    [
      "prose, quoting it in the error",
      "Done! Everything passes.",
      "agent 'fix' output: response is neither JSON nor one ```json block; the response starts \"Done! Everything passes.\"",
    ],
    [
      "two fenced blocks",
      '```json\n{"fixed":[]}\n```\n```json\n{"fixed":["b"]}\n```',
      "response has 2 fenced blocks; expected exactly one",
    ],
    [
      "a fenced block that is not JSON",
      "```json\n{fixed: []}\n```",
      "the fenced block is not JSON",
    ],
    [
      "a fenced block of another language",
      "```ts\nconst a = 1;\n```",
      "response is neither JSON nor one ```json block",
    ],
  ])("rejects %s", async (_name, text, message) => {
    await expect(structured.decode({ text }, op)).rejects.toThrow(message);
  });

  it("quotes only the first 200 characters of a long response", async () => {
    await expect(structured.decode({ text: "x".repeat(500) }, op)).rejects.toThrow(/"x{200}…"/);
  });

  it("rejects JSON that does not match the output schema with a SchemaError", async () => {
    await expect(structured.decode({ text: '{"fixed":1}' }, op)).rejects.toBeInstanceOf(
      SchemaError,
    );
  });

  it.each([[{ stdout: "x" }], [undefined]])("rejects the completion output %j", async (output) => {
    await expect(fixTests.decode(output, dummyOp)).rejects.toThrow(
      "agent completion did not contain string text",
    );
  });
});

describe("agent failures", () => {
  it("surfaces a failed completion as an OperationError with its message and phase", async () => {
    const details = { phase: "execute", repositoryMayBeModified: true };
    const executor = {
      execute: async (r: OperationRequest) => ({
        protocolVersion: PROTOCOL_VERSION,
        commandId: r.commandId,
        status: "failed" as const,
        error: { message: "model error", details },
      }),
    };
    const error = await run(
      dynamic(() => fixTests({ input: { packages: ["web"] } })),
      { executor },
    ).catch((e: unknown) => e);
    expect([error instanceof OperationError, (error as OperationError).detail]).toEqual([
      true,
      { message: "model error", details },
    ]);
  });
});

describe("process-tree cleanup", () => {
  it("lists descendants by walking parent ids, parents first", () => {
    const table = new Map([
      [14, 12],
      [10, 1],
      [12, 11],
      [11, 10],
      [13, 1],
      [15, 10],
    ]);
    expect(descendants(10, table)).toEqual([11, 15, 12, 14]);
  });

  it("stops parents first and kills children first, then the group, from one listing", () => {
    const signals: [number, string][] = [];
    const kill = vi.spyOn(process, "kill").mockImplementation((pid, name) => {
      signals.push([pid, String(name)]);
      return true;
    });
    let listings = 0;
    try {
      killProcessTree(100, "darwin", () => {
        listings += 1;
        return {
          // 101 stays in the child's group; 102 (the bash tool's session) and
          // its child 103 moved to group 102; 200 is unrelated.
          parents: new Map([
            [100, 1],
            [101, 100],
            [102, 100],
            [103, 102],
            [200, 1],
          ]),
          groups: new Map([
            [100, 100],
            [101, 100],
            [102, 102],
            [103, 102],
            [200, 200],
          ]),
        };
      });
    } finally {
      kill.mockRestore();
    }
    expect([listings, signals]).toEqual([
      1,
      [
        [-100, "SIGSTOP"],
        [100, "SIGSTOP"],
        [102, "SIGSTOP"],
        [103, "SIGSTOP"],
        [103, "SIGKILL"],
        [102, "SIGKILL"],
        [-100, "SIGKILL"],
        [100, "SIGKILL"],
      ],
    ]);
  });
});

describe("agent launch environment", () => {
  it("passes allowlisted host variables plus explicit ones, and no other credentials", () => {
    const host = {
      PATH: "/bin",
      HOME: "/home/dev",
      LC_ALL: "C",
      https_proxy: "http://proxy:8080",
      LLM_API_KEY: "llm-key",
      LLM_MODEL: "gpt-4o",
      DECISION_API_KEY: "decision-key",
      AWS_SECRET_ACCESS_KEY: "aws",
      GITHUB_TOKEN: "gh",
      NPM_TOKEN: "npm",
    };
    expect(agentEnvironment(host, { EXTRA: "1" }, "linux")).toEqual({
      PATH: "/bin",
      HOME: "/home/dev",
      LC_ALL: "C",
      https_proxy: "http://proxy:8080",
      LLM_API_KEY: "llm-key",
      LLM_MODEL: "gpt-4o",
      EXTRA: "1",
    });
  });

  it("lets an explicit variable replace a host variable of another case on Windows", () => {
    const host = { Path: "C:\\host", SystemRoot: "C:\\Windows" };
    expect(agentEnvironment(host, { PATH: "C:\\explicit" }, "win32")).toEqual({
      PATH: "C:\\explicit",
      SystemRoot: "C:\\Windows",
    });
  });

  it("keeps variables whose names differ only in case on other platforms", () => {
    expect(agentEnvironment({ Path: "a" }, { PATH: "b" }, "linux")).toEqual({
      Path: "a",
      PATH: "b",
    });
  });

  it("hands the builtin runner its LLM key on stdin, explicit over host on Windows", () => {
    const host = { Path: "C:\\host", llm_api_key: "lower" };
    expect(agentLaunch(host, { LLM_API_KEY: "upper" }, "win32")).toEqual({
      env: { Path: "C:\\host", CODEMOD_AGENT_SECRETS: "stdin" },
      stdin: JSON.stringify({ LLM_API_KEY: "upper" }),
    });
  });

  it("omits a blank LLM key", () => {
    expect(agentLaunch({ PATH: "/bin", LLM_API_KEY: " " }, {}, "linux")).toEqual({
      env: { PATH: "/bin" },
    });
  });

  it.each(["claude-code", "codex"] as const)(
    "gives %s its CLI home but no LLM settings or provider secrets",
    (backend) => {
      const host = {
        PATH: "/bin",
        HOME: "/home/dev",
        CLAUDE_CONFIG_DIR: "/home/dev/.claude-work",
        CODEX_HOME: "/home/dev/.codex-work",
        LLM_API_KEY: "llm-key",
        LLM_PROVIDER: "openai",
        llm_model: "gpt-4o",
        ANTHROPIC_API_KEY: "anthropic",
        OPENAI_API_KEY: "openai",
        DECISION_API_KEY: "decision",
      };
      expect(agentLaunch(host, {}, "linux", backend)).toEqual({
        env: {
          PATH: "/bin",
          HOME: "/home/dev",
          CLAUDE_CONFIG_DIR: "/home/dev/.claude-work",
          CODEX_HOME: "/home/dev/.codex-work",
        },
      });
    },
  );

  // The executor refuses these first (externalAgentEnvProblem); the launch strips them regardless.
  it("strips credential-looking explicit variables from an external runner's launch", () => {
    const explicit = { ANTHROPIC_API_KEY: "explicit", GITHUB_TOKEN: "gh", EXTRA: "1" };
    expect(agentLaunch({ PATH: "/bin" }, explicit, "linux", "claude-code")).toEqual({
      env: { PATH: "/bin", EXTRA: "1" },
    });
  });

  it.each([
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CODEX_API_KEY",
    "AWS_PROFILE",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GITHUB_TOKEN",
    "NPM_TOKEN",
    "db_password",
    "SSH_AUTH_SOCK",
    "MY_SECRET",
    "STRIPE_KEY",
    "LLM_API_KEY",
    "CODEMOD_AGENT_SECRETS",
  ])("classifies %s as a credential", (name) => {
    expect(isSecretEnvName(name)).toBe(true);
  });

  it.each([
    "PATH",
    "HOME",
    "CLAUDE_CONFIG_DIR",
    "codex_home",
    "FAKE_AGENT_MODE",
    "KEYBOARD",
    "MONKEY",
  ])("does not classify %s as a credential", (name) => {
    expect(isSecretEnvName(name)).toBe(false);
  });

  it("names the credential variables an external backend refuses", () => {
    expect(externalAgentEnvProblem({ OPENAI_API_KEY: "sk-test", EXTRA: "1" })).toContain(
      "(OPENAI_API_KEY)",
    );
  });

  it("accepts CLI home variables for an external backend", () => {
    expect(externalAgentEnvProblem({ CLAUDE_CONFIG_DIR: "/x", CODEX_HOME: "/y" })).toBeUndefined();
  });
});

describe("agent history", () => {
  let repo: string;
  const store = new MemoryHistoryStore();
  const workflow = (backend: Backend) =>
    dynamic(
      async () =>
        (await agent({ name: "review", prompt: "Review the diff.", backend })()).text.length > 0,
    );
  const recorded = () => MemoryHistoryStore.fromJSON(store.serialize());
  const offline = {
    execute: (r: OperationRequest) => executeAgent({ bin: undefined, cwd: repo }, r),
  };

  // One real run through the fake runner records the history every test replays.
  beforeAll(async () => {
    repo = mkdtempSync(join(tmpdir(), "agent-history-"));
    const executor = {
      execute: (r: OperationRequest, signal?: AbortSignal) =>
        executeAgent({ bin: fakeAgent, cwd: repo, hostEnv: { PATH: process.env.PATH } }, r, signal),
    };
    await run(workflow({ kind: "codex", sandbox: "read-only" }), { executor, history: store });
  });
  afterAll(() => rmSync(repo, { recursive: true, force: true }));

  it("records the step's backend with the scheduled command", () => {
    expect(store.toJSON().events[0]).toMatchObject({
      type: "scheduled",
      command: {
        id: "review",
        operation: { kind: "agent", backend: { kind: "codex", sandbox: "read-only" } },
      },
    });
  });

  it("replays a recorded step without an agent runner", async () => {
    const replay = await run(workflow({ kind: "codex", sandbox: "read-only" }), {
      executor: offline,
      history: recorded(),
    });
    expect(replay.replayed).toBe(true);
  });

  it.each<Backend>([
    { kind: "codex", sandbox: "workspace-write" },
    { kind: "claude-code", tools: ["Read"] },
    { kind: "builtin" },
  ])("treats a switch to backend %j as a changed command on replay", async (backend) => {
    const error = await run(workflow(backend), { executor: offline, history: recorded() }).catch(
      (e: unknown) => e,
    );
    expect([error instanceof NondeterminismError, (error as NondeterminismError).kind]).toEqual([
      true,
      "changed",
    ]);
  });
});

describe("agent composition", () => {
  it("takes a preceding step's output as input and decodes the harness's scripted text", async () => {
    const discover = shell({ name: "discover", command: "true", output: Packages });
    const h = createHarness({
      results: { discover: { packages: ["web"] }, "fix-tests": "Fixed web." },
    });
    const result = await h.run(sequence(discover(), fixTests()));
    expect([h.executed[1]!.operation, result.output]).toEqual([
      {
        kind: "agent",
        prompt: "Fix the failing tests in the listed packages.",
        input: { packages: ["web"] },
        backend: builtin(),
      },
      { text: "Fixed web." },
    ]);
  });
});

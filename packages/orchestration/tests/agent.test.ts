/**
 * Mocked contract tests for `agent()`: request serialization, response
 * parsing, validation and failure behavior, the launch environment
 * (`agent-env.ts`), and process-tree cleanup. Execution through the
 * `codemod-agent` runner is `agent-runner.test.ts`. No model is called; the
 * live checks are `agent.*.live.test.ts`.
 */
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { canListProcesses, descendants, killProcessTree } from "../src/execution/process-tree.ts";
import { createHarness, failed } from "../src/host/harness.ts";
import {
  BUILTIN_AGENT_TOOLS,
  CLAUDE_CODE_TOOLS,
  CollectingSink,
  DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS,
  CODEX_SANDBOXES,
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
  canonicalJson,
  externalAgentEnvProblem,
  isSecretEnvName,
  dynamic,
  executeAgent,
  guard,
  isOperation,
  isOperationRequest,
  parseAgentJson,
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
  it("builds the shared fixture operations for every backend", () => {
    const structured = agent({
      name: "fix-tests",
      prompt: "Fix the failing tests in the listed packages. Report the fixed files.",
      input: Packages,
      output: Fixed,
      backend: { kind: "builtin", maxSteps: 40 },
    });
    const claude = agent({
      name: "summarize-readme",
      prompt: "Summarize README.md in one sentence.",
      backend: { kind: "claude-code", tools: ["Read", "Glob", "Grep"] },
    });
    const Version = guard("version", (v): v is { version: string } => typeof v === "object");
    const codex = agent({
      name: "bump-version",
      prompt: "Set the version in VERSION to 2.0.0.",
      input: Version,
      output: Version,
      backend: { kind: "codex" },
    });
    for (const [fixtureName, commandId, op] of [
      ["agent-request.json", "fix-tests", structured.toOperation({ packages: ["web", "api"] })],
      ["agent-claude-code-request.json", "summarize-readme", claude.toOperation()],
      ["agent-codex-request.json", "bump-version", codex.toOperation({ version: "2.0.0" })],
    ] as const) {
      const request = { protocolVersion: PROTOCOL_VERSION, commandId, operation: op };
      expect(isOperationRequest(request), fixtureName).toBe(true);
      expect(canonicalJson(request), fixtureName).toBe(canonicalJson(fixture(fixtureName)));
    }
    expect(structured.kind).toBe("agent");
  });

  it("defaults to the builtin backend with an explicit, recorded tool set without bash", () => {
    const tidy = agent({ name: "tidy", prompt: "Tidy the README." });
    expect(tidy.toOperation()).toEqual({
      kind: "agent",
      prompt: "Tidy the README.",
      backend: {
        kind: "builtin",
        tools: [
          "str_replace_based_edit_tool",
          "json_edit_tool",
          "glob",
          "sequentialthinking",
          "task_done",
        ],
      },
    });
    expect(
      agent({ name: "t", prompt: "Tidy the README.", backend: { kind: "builtin" } }).toOperation(),
    ).toEqual(tidy.toOperation());
    for (const unconfined of ["bash", "mcp_tool", "ckg_tool"] as const) {
      expect(DEFAULT_BUILTIN_AGENT_TOOLS).not.toContain(unconfined);
    }
    expect(DEFAULT_CLAUDE_CODE_TOOLS).not.toContain("Bash");
    expect(BUILTIN_AGENT_TOOLS).toContain("bash");
    expect(CLAUDE_CODE_TOOLS).toContain("Bash");
    expect([...CODEX_SANDBOXES]).toEqual(["read-only", "workspace-write"]);
  });

  it("records each backend's own settings and fills its safe defaults", () => {
    const op = (backend: Parameters<typeof agent>[0]["backend"]) =>
      agent({ name: "s", prompt: "p", backend }).toOperation();
    expect(op({ kind: "builtin", tools: ["bash", "glob"], maxSteps: 5 })).toEqual(
      operation({ backend: builtin({ tools: ["bash", "glob"], maxSteps: 5 }) }),
    );
    expect(op({ kind: "builtin", tools: [] })).toEqual(
      operation({ backend: builtin({ tools: [] }) }),
    );
    expect(op({ kind: "claude-code" })).toEqual(
      operation({
        backend: { kind: "claude-code", tools: ["Read", "Edit", "Write", "Glob", "Grep"] },
      }),
    );
    expect(op({ kind: "claude-code", tools: ["Read", "Bash"] })).toEqual(
      operation({ backend: { kind: "claude-code", tools: ["Read", "Bash"] } }),
    );
    expect(op({ kind: "codex" })).toEqual(
      operation({ backend: { kind: "codex", sandbox: "workspace-write" } }),
    );
    expect(op({ kind: "codex", sandbox: "read-only" })).toEqual(
      operation({ backend: { kind: "codex", sandbox: "read-only" } }),
    );
  });

  it("refuses settings a backend cannot enforce when the step is defined", () => {
    const define =
      (backend: unknown, extra: Record<string, unknown> = {}) =>
      () =>
        agent({ name: "x", prompt: "p", backend, ...extra } as never);
    const cases: [unknown, Record<string, unknown>, string | RegExp][] = [
      [
        { kind: "builtin", tools: ["rm_rf"] },
        {},
        /agent 'x': builtin tools must be distinct names from 'bash'/,
      ],
      [{ kind: "builtin", tools: ["glob", "glob"] }, {}, /builtin tools must be distinct/],
      [
        { kind: "builtin", maxSteps: 0 },
        {},
        "agent 'x': builtin maxSteps must be a positive integer",
      ],
      [{ kind: "builtin", maxSteps: 1.5 }, {}, /maxSteps/],
      [
        { kind: "builtin", sandbox: "read-only" },
        {},
        "agent 'x': backend 'builtin' does not support 'sandbox'",
      ],
      [
        { kind: "claude-code", maxSteps: 3 },
        {},
        "agent 'x': backend 'claude-code' does not support 'maxSteps'",
      ],
      [
        { kind: "claude-code", tools: ["bash"] },
        {},
        /claude-code tools must be distinct names from 'Read'/,
      ],
      [{ kind: "claude-code", tools: ["Read", "Read"] }, {}, /claude-code tools must be distinct/],
      [
        { kind: "codex", tools: ["Read"] },
        {},
        "agent 'x': backend 'codex' does not support 'tools'",
      ],
      [
        { kind: "codex", maxSteps: 3 },
        {},
        "agent 'x': backend 'codex' does not support 'maxSteps'",
      ],
      [
        { kind: "codex", sandbox: "danger-full-access" },
        {},
        /codex sandbox must be one of 'read-only', 'workspace-write'/,
      ],
      [{ kind: "opencode" }, {}, /backend kind must be one of 'builtin', 'claude-code', 'codex'/],
      [null, {}, "agent 'x': backend must be an object"],
      // Pre-backend options are not aliases.
      [
        undefined,
        { tools: ["glob"] },
        /unknown option 'tools'; backend settings such as tools belong in 'backend'/,
      ],
      [undefined, { maxSteps: 3 }, /unknown option 'maxSteps'/],
    ];
    for (const [backend, extra, message] of cases) {
      expect(define(backend, extra), JSON.stringify(backend)).toThrow(message);
    }
  });

  it("validates agent operations strictly and no longer knows 'ai'", () => {
    expect(
      isOperation(
        operation({ input: { a: 1 }, backend: builtin({ maxSteps: 3 }), responseFormat: "json" }),
      ),
    ).toBe(true);
    expect(isOperation(operation({ backend: { kind: "claude-code", tools: [] } }))).toBe(true);
    expect(isOperation(operation({ backend: { kind: "codex", sandbox: "read-only" } }))).toBe(true);
    expect(isOperation({ kind: "agent", prompt: "p" })).toBe(false);
    expect(isOperation({ kind: "agent", prompt: "p", tools: [] })).toBe(false);
    expect(isOperation(operation({ backend: builtin({ tools: ["shell"] }) }))).toBe(false);
    expect(isOperation(operation({ backend: builtin({ tools: ["glob", "glob"] }) }))).toBe(false);
    expect(isOperation(operation({ backend: builtin({ maxSteps: 0 }) }))).toBe(false);
    expect(
      isOperation(operation({ backend: { kind: "claude-code", tools: ["Read"], maxSteps: 2 } })),
    ).toBe(false);
    expect(isOperation(operation({ backend: { kind: "codex" } }))).toBe(false);
    expect(
      isOperation(operation({ backend: { kind: "codex", sandbox: "danger-full-access" } })),
    ).toBe(false);
    expect(isOperation(operation({ responseFormat: "xml" }))).toBe(false);
    expect(isOperation(operation({ input: Number.NaN }))).toBe(false);
    expect(isOperation(operation({ model: "x" }))).toBe(false);
    expect(isOperation({ kind: "ai", prompt: "p" })).toBe(false);
  });
});

describe("agent response parsing", () => {
  const structured = agent({ name: "fix", prompt: "Report fixed files.", output: Fixed });
  /** A dummy operation to satisfy the decode signature; agent decode ignores it. */
  const dummyOp = fixTests.toOperation({ packages: [] });

  it("returns the final response as { text } without an output schema", async () => {
    await expect(fixTests.decode({ text: "Fixed 2 tests." }, dummyOp)).resolves.toEqual({
      text: "Fixed 2 tests.",
    });
    expect(fixTests.toOperation({ packages: [] })).not.toHaveProperty("responseFormat");
  });

  it("asks for JSON and parses bare JSON or a single fenced block", async () => {
    const op = structured.toOperation();
    expect(op).toMatchObject({ responseFormat: "json" });
    const cases = [
      '{"fixed":["a.test.ts"]}',
      '  {"fixed":["a.test.ts"]}\n',
      '```json\n{"fixed":["a.test.ts"]}\n```',
      'Done. Here is the result:\n\n```json\n{\n  "fixed": ["a.test.ts"]\n}\n```\n',
      '```\n{"fixed":["a.test.ts"]}\n```',
    ];
    for (const text of cases) {
      const result = await structured.decode({ text }, op);
      const files: string[] = result.fixed;
      expect(files, text).toEqual(["a.test.ts"]);
    }
  });

  it("rejects output that does not match the contract, with the response in the error", async () => {
    const op = structured.toOperation();
    await expect(structured.decode({ text: "Done! Everything passes." }, op)).rejects.toThrow(
      "agent 'fix' output: response is neither JSON nor one ```json block; the response starts \"Done! Everything passes.\"",
    );
    await expect(
      structured.decode({ text: '```json\n{"fixed":[]}\n```\n```json\n{"fixed":["b"]}\n```' }, op),
    ).rejects.toThrow("response has 2 fenced blocks; expected exactly one");
    await expect(structured.decode({ text: "```json\n{fixed: []}\n```" }, op)).rejects.toThrow(
      /the fenced block is not JSON/,
    );
    await expect(structured.decode({ text: '{"fixed":1}' }, op)).rejects.toBeInstanceOf(
      SchemaError,
    );
    await expect(structured.decode({ text: "x".repeat(500) }, op)).rejects.toThrow(/x{200}…"/);
    await expect(fixTests.decode({ stdout: "x" }, dummyOp)).rejects.toThrow(
      "agent completion did not contain string text",
    );
    await expect(fixTests.decode(undefined, dummyOp)).rejects.toThrow(
      "agent completion did not contain string text",
    );
    expect(parseAgentJson("```ts\nconst a = 1;\n```")).toEqual({
      error: "response is neither JSON nor one ```json block",
    });
  });
});

const isAlive = (pid: number): boolean => {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
};

async function waitFor<T>(check: () => T | undefined, timeoutMs = 5_000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = check();
    if (value !== undefined) return value;
    if (Date.now() > deadline) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 25));
  }
}

describe("agent failures and plumbing", () => {
  let repo: string;
  beforeEach(() => {
    repo = mkdtempSync(join(tmpdir(), "agent-plumbing-"));
  });
  afterEach(() => rmSync(repo, { recursive: true, force: true }));

  const request: OperationRequest = {
    protocolVersion: PROTOCOL_VERSION,
    commandId: "fix-tests",
    operation: { kind: "agent", prompt: "x", backend: { kind: "builtin", tools: [] } },
  };
  const externalRequests: OperationRequest[] = [
    {
      ...request,
      operation: { kind: "agent", prompt: "x", backend: { kind: "claude-code", tools: [] } },
    },
    {
      ...request,
      operation: { kind: "agent", prompt: "x", backend: { kind: "codex", sandbox: "read-only" } },
    },
  ];

  it("surfaces a failed agent completion, with its phase, as an OperationError", async () => {
    const details = { phase: "execute", repositoryMayBeModified: true };
    const h = createHarness({ results: { "fix-tests": failed("model error") } });
    const error = await h
      .run(dynamic(() => fixTests({ input: { packages: ["web"] } })))
      .catch((e: unknown) => e);
    expect(error).toBeInstanceOf(OperationError);
    expect((error as OperationError).detail).toEqual({ message: "model error" });

    const executor = {
      execute: async (r: OperationRequest) => ({
        protocolVersion: PROTOCOL_VERSION,
        commandId: r.commandId,
        status: "failed" as const,
        error: { message: "model error", details },
      }),
    };
    const detailed = await run(
      dynamic(() => fixTests({ input: { packages: ["web"] } })),
      { executor },
    ).catch((e: unknown) => e);
    expect((detailed as OperationError).detail).toEqual({ message: "model error", details });
  });

  it("walks descendants through parent ids, parents first", () => {
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

  it("kills from one snapshot: stops parents first, kills children first, then the group", () => {
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
    expect(listings).toBe(1);
    expect(signals).toEqual([
      [-100, "SIGSTOP"],
      [100, "SIGSTOP"],
      [102, "SIGSTOP"],
      [103, "SIGSTOP"],
      [103, "SIGKILL"],
      [102, "SIGKILL"],
      [-100, "SIGKILL"],
      [100, "SIGKILL"],
    ]);
  });

  it("dedupes environment names case-insensitively on Windows only, and omits an empty key", () => {
    const host = { Path: "C:\\host", SystemRoot: "C:\\Windows", llm_api_key: "lower" };
    expect(agentEnvironment(host, { PATH: "C:\\explicit" }, "win32")).toEqual({
      PATH: "C:\\explicit",
      SystemRoot: "C:\\Windows",
      llm_api_key: "lower",
    });
    expect(agentEnvironment({ Path: "a" }, { PATH: "b" }, "linux")).toEqual({
      Path: "a",
      PATH: "b",
    });
    expect(agentLaunch(host, { LLM_API_KEY: "upper" }, "win32")).toEqual({
      env: { Path: "C:\\host", SystemRoot: "C:\\Windows", CODEMOD_AGENT_SECRETS: "stdin" },
      stdin: JSON.stringify({ LLM_API_KEY: "upper" }),
    });
    expect(agentLaunch({ PATH: "/bin", LLM_API_KEY: " " }, {}, "linux")).toEqual({
      env: { PATH: "/bin" },
    });
  });

  it("gives external backends their CLI home but no LLM settings or secret", async () => {
    const host = {
      PATH: process.env.PATH,
      HOME: "/home/dev",
      CLAUDE_CONFIG_DIR: "/home/dev/.claude-work",
      CODEX_HOME: "/home/dev/.codex-work",
      LLM_API_KEY: "llm-key",
      LLM_PROVIDER: "openai",
      llm_model: "gpt-4o",
      ANTHROPIC_API_KEY: "anthropic",
      OPENAI_API_KEY: "openai",
      TYPESAFE_API_KEY: "typesafe",
    };
    for (const backend of ["claude-code", "codex"] as const) {
      expect(agentLaunch(host, {}, "linux", backend)).toEqual({
        env: {
          PATH: process.env.PATH,
          HOME: "/home/dev",
          CLAUDE_CONFIG_DIR: "/home/dev/.claude-work",
          CODEX_HOME: "/home/dev/.codex-work",
        },
      });
    }
    // Credential-looking explicit variables never reach an external runner:
    // the executor refuses them, and the launch strips them regardless.
    const stripped = agentLaunch(
      host,
      { ANTHROPIC_API_KEY: "explicit", GITHUB_TOKEN: "gh", EXTRA: "1" },
      "linux",
      "claude-code",
    ).env;
    expect(stripped).toMatchObject({ EXTRA: "1" });
    expect(stripped).not.toHaveProperty("ANTHROPIC_API_KEY");
    expect(stripped).not.toHaveProperty("GITHUB_TOKEN");
  });

  it("allowlists the launch environment plus explicit variables", async () => {
    const host = {
      PATH: process.env.PATH,
      HOME: "/home/dev",
      LC_ALL: "C",
      https_proxy: "http://proxy:8080",
      LLM_API_KEY: "llm-key",
      LLM_MODEL: "gpt-4o",
      TYPESAFE_API_KEY: "typesafe-key",
      AWS_SECRET_ACCESS_KEY: "aws",
      GITHUB_TOKEN: "gh",
      NPM_TOKEN: "npm",
    };
    expect(agentEnvironment(host, { EXTRA: "1" }, process.platform)).toEqual({
      PATH: process.env.PATH,
      HOME: "/home/dev",
      LC_ALL: "C",
      https_proxy: "http://proxy:8080",
      LLM_API_KEY: "llm-key",
      LLM_MODEL: "gpt-4o",
      EXTRA: "1",
    });
  });

  it("classifies credential-looking variable names and refuses them for external backends", async () => {
    for (const name of [
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
    ]) {
      expect(isSecretEnvName(name), name).toBe(true);
    }
    for (const name of [
      "PATH",
      "HOME",
      "CLAUDE_CONFIG_DIR",
      "codex_home",
      "FAKE_AGENT_MODE",
      "KEYBOARD",
      "MONKEY",
    ]) {
      expect(isSecretEnvName(name), name).toBe(false);
    }

    expect(externalAgentEnvProblem({ OPENAI_API_KEY: "sk-test", EXTRA: "1" })).toContain(
      "(OPENAI_API_KEY)",
    );
    expect(externalAgentEnvProblem({ CLAUDE_CONFIG_DIR: "/x", CODEX_HOME: "/y" })).toBeUndefined();
  });

  it("records the backend in history, replays it offline, and treats a backend change as changed", async () => {
    const executor = {
      execute: (r: OperationRequest, signal?: AbortSignal) =>
        executeAgent({ bin: fakeAgent, cwd: repo, hostEnv: { PATH: process.env.PATH } }, r, signal),
    };
    const step = (backend: Parameters<typeof agent>[0]["backend"]) =>
      agent({ name: "review", prompt: "Review the diff.", backend });
    const workflow = (backend: Parameters<typeof agent>[0]["backend"]) =>
      dynamic(async () => (await step(backend)()).text.length > 0);
    const store = new MemoryHistoryStore();
    await run(workflow({ kind: "codex", sandbox: "read-only" }), { executor, history: store });
    expect(store.toJSON().events[0]).toMatchObject({
      type: "scheduled",
      command: {
        id: "review",
        operation: { kind: "agent", backend: { kind: "codex", sandbox: "read-only" } },
      },
    });

    const offline = {
      execute: (r: OperationRequest) => executeAgent({ bin: undefined, cwd: repo }, r),
    };
    const replay = await run(workflow({ kind: "codex", sandbox: "read-only" }), {
      executor: offline,
      history: store,
    });
    expect(replay.replayed).toBe(true);

    for (const changed of [
      { kind: "codex", sandbox: "workspace-write" },
      { kind: "claude-code", tools: ["Read"] },
      { kind: "builtin" },
    ] as const) {
      const error = await run(workflow(changed), {
        executor: offline,
        history: MemoryHistoryStore.fromJSON(store.serialize()),
      }).catch((e: unknown) => e);
      expect(error, JSON.stringify(changed)).toBeInstanceOf(NondeterminismError);
      expect((error as NondeterminismError).kind).toBe("changed");
    }
  });

  it("composes statically and is scriptable in the harness", async () => {
    const discover = shell({ name: "discover", command: "true", output: Packages });
    const h = createHarness({
      results: { discover: { packages: ["web"] }, "fix-tests": "Fixed web." },
    });
    const result = await h.run(sequence(discover(), fixTests()));
    expect(result.output).toEqual({ text: "Fixed web." });
    expect(h.executed[1]!.operation).toEqual({
      kind: "agent",
      prompt: "Fix the failing tests in the listed packages.",
      input: { packages: ["web"] },
      backend: builtin(),
    });
  });
});

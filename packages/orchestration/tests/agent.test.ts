/**
 * Mocked contract tests for `agent()`: request serialization, response
 * parsing, validation and failure behavior, and execution plumbing through
 * the bridge seam (`tests/fixtures/fake-bridge.mjs` stands in for the Rust
 * binary): environment allowlisting, abort semantics, and process-tree
 * cleanup. No model is called; the live check is `agent.live.test.ts`.
 */
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { canListProcesses, descendants, killProcessTree } from "../src/execution/process-tree.ts";
import { createHarness, failed } from "../src/host/harness.ts";
import {
  BUILTIN_AGENT_TOOLS,
  BridgeExecutor,
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
const fakeBridge = resolve(import.meta.dirname, "fixtures/fake-bridge.mjs");

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

  it("reports a bridge that cannot start as unknown, since an agent may change files", async () => {
    const executor = new BridgeExecutor({ bin: join(repo, "missing-bridge"), cwd: repo });
    const completion = await executor.execute(request);
    expect(completion.status).toBe("unknown");
    expect(completion.error?.details).toEqual({ phase: "bridge", repositoryMayBeModified: true });
  });

  it("is cancelled with nothing modified only when aborted before the bridge starts", async () => {
    const executor = new BridgeExecutor({ bin: fakeBridge, cwd: repo });
    for (const r of [request, ...externalRequests]) {
      const completion = await executor.execute(r, AbortSignal.abort());
      expect(completion).toMatchObject({
        status: "cancelled",
        error: { details: { phase: "start", repositoryMayBeModified: false } },
      });
    }
  });

  it.each([
    ["builtin", request],
    ["claude-code", externalRequests[0]!],
    ["codex", externalRequests[1]!],
  ] as const)(
    "is unknown when a %s agent is aborted after spawn, and kills the processes the bridge started",
    async (_backend, request) => {
      const pidFile = join(repo, "pids.json");
      const executor = new BridgeExecutor({
        bin: fakeBridge,
        cwd: repo,
        env: { FAKE_BRIDGE_MODE: "hang", FAKE_PID_FILE: pidFile, FAKE_CHILDREN: "2" },
      });
      const controller = new AbortController();
      const pending = executor.execute(request, controller.signal);
      const children = await waitFor(() =>
        existsSync(pidFile) ? (JSON.parse(readFileSync(pidFile, "utf8")) as number[]) : undefined,
      );
      expect(children.every(isAlive)).toBe(true);
      controller.abort();
      const completion = await pending;
      expect(completion.status).toBe("unknown");
      expect(completion.error).toEqual({
        message: expect.stringMatching(/killed by SIGKILL on abort|aborted while/),
        details: { phase: "execute", repositoryMayBeModified: true },
      });
      await waitFor(() => (children.some(isAlive) ? undefined : true));
    },
  );

  it.skipIf(process.platform === "win32" || !canListProcesses())(
    "also kills children that left the bridge's process group",
    async () => {
      const pidFile = join(repo, "pids.json");
      const executor = new BridgeExecutor({
        bin: fakeBridge,
        cwd: repo,
        env: { FAKE_BRIDGE_MODE: "hang", FAKE_PID_FILE: pidFile, FAKE_DETACH_CHILDREN: "1" },
      });
      const controller = new AbortController();
      const pending = executor.execute(request, controller.signal);
      const children = await waitFor(() =>
        existsSync(pidFile) ? (JSON.parse(readFileSync(pidFile, "utf8")) as number[]) : undefined,
      );
      controller.abort();
      expect((await pending).status).toBe("unknown");
      await waitFor(() => (children.some(isAlive) ? undefined : true));
    },
  );

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
          // 101 stays in the bridge's group; 102 (the bash tool's session) and
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
      env: { Path: "C:\\host", SystemRoot: "C:\\Windows", CODEMOD_BRIDGE_SECRETS: "stdin" },
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
    // Credential-looking explicit variables never reach an external bridge:
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

    const executor = new BridgeExecutor({ bin: fakeBridge, cwd: repo, hostEnv: host });
    for (const external of externalRequests) {
      const completion = await executor.execute(external);
      const { env, secrets } = JSON.parse((completion.output as { text: string }).text) as {
        env: Record<string, string>;
        secrets: unknown;
      };
      expect(secrets).toBeNull();
      expect(env).toMatchObject({ HOME: "/home/dev", CODEX_HOME: "/home/dev/.codex-work" });
      for (const name of [
        "LLM_API_KEY",
        "LLM_PROVIDER",
        "llm_model",
        "CODEMOD_BRIDGE_SECRETS",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "TYPESAFE_API_KEY",
      ]) {
        expect(env).not.toHaveProperty(name);
      }
    }
  });

  it("starts the bridge with an allowlisted environment plus explicit variables", async () => {
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
    expect(agentEnvironment(host, { EXTRA: "1" })).toEqual({
      PATH: process.env.PATH,
      HOME: "/home/dev",
      LC_ALL: "C",
      https_proxy: "http://proxy:8080",
      LLM_API_KEY: "llm-key",
      LLM_MODEL: "gpt-4o",
      EXTRA: "1",
    });

    const executor = new BridgeExecutor({
      bin: fakeBridge,
      cwd: repo,
      hostEnv: host,
      env: { EXTRA: "1" },
    });
    const completion = await executor.execute(request);
    const { env, secrets } = JSON.parse((completion.output as { text: string }).text) as {
      env: Record<string, string>;
      secrets: unknown;
    };
    expect(env).toMatchObject({ EXTRA: "1", HOME: "/home/dev", CODEMOD_BRIDGE_SECRETS: "stdin" });
    // The key reaches the bridge on stdin, never in its launch environment.
    expect(env).not.toHaveProperty("LLM_API_KEY");
    expect(secrets).toEqual({ LLM_API_KEY: "llm-key" });
    for (const secret of [
      "TYPESAFE_API_KEY",
      "AWS_SECRET_ACCESS_KEY",
      "GITHUB_TOKEN",
      "NPM_TOKEN",
    ]) {
      expect(env).not.toHaveProperty(secret);
    }
  });

  it("sends the request to the bridge in the target directory, records it, and replays", async () => {
    const executor = new BridgeExecutor({ bin: fakeBridge, cwd: repo });
    const store = new MemoryHistoryStore();
    const workflow = dynamic(async () => {
      const { text } = await fixTests({ input: { packages: ["web"] } });
      const echoed = JSON.parse(text) as { request: OperationRequest; cwd: string };
      return { request: echoed.request, cwd: echoed.cwd };
    });
    const first = await run(workflow, { executor, history: store });
    expect(first.output.request.operation).toEqual({
      kind: "agent",
      prompt: "Fix the failing tests in the listed packages.",
      input: { packages: ["web"] },
      backend: builtin(),
    });
    expect(realpathSync(first.output.cwd)).toBe(realpathSync(repo));
    expect(store.toJSON().events.map((e) => e.type)).toEqual([
      "scheduled",
      "completed",
      "finalized",
    ]);

    const offline = new BridgeExecutor({ bin: join(repo, "missing-bridge"), cwd: repo });
    const replay = await run(workflow, { executor: offline, history: store });
    expect(replay.output).toEqual(first.output);
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
      "CODEMOD_BRIDGE_SECRETS",
    ]) {
      expect(isSecretEnvName(name), name).toBe(true);
    }
    for (const name of [
      "PATH",
      "HOME",
      "CLAUDE_CONFIG_DIR",
      "codex_home",
      "FAKE_BRIDGE_MODE",
      "KEYBOARD",
      "MONKEY",
    ]) {
      expect(isSecretEnvName(name), name).toBe(false);
    }

    const events = new CollectingSink();
    for (const external of externalRequests) {
      const executor = new BridgeExecutor({
        bin: fakeBridge,
        cwd: repo,
        events,
        env: { OPENAI_API_KEY: "sk-test", EXTRA: "1" },
      });
      const completion = await executor.execute(external);
      expect(completion).toEqual({
        protocolVersion: PROTOCOL_VERSION,
        commandId: "fix-tests",
        status: "failed",
        error: {
          message: expect.stringContaining("(OPENAI_API_KEY)"),
          details: { phase: "config", repositoryMayBeModified: false },
        },
      });
      expect(JSON.stringify(completion)).not.toContain("sk-test");
    }
    expect(events.events.filter((e) => e.type === "bridge.spawned")).toHaveLength(0);
    // The builtin backend keeps its documented behavior.
    const builtinCompletion = await new BridgeExecutor({
      bin: fakeBridge,
      cwd: repo,
      env: { OPENAI_API_KEY: "sk-test" },
    }).execute(request);
    expect(builtinCompletion.status).toBe("succeeded");
    expect(externalAgentEnvProblem({ CLAUDE_CONFIG_DIR: "/x", CODEX_HOME: "/y" })).toBeUndefined();
  });

  it("times external agents out on the host clock, killing the process tree", async () => {
    const pidFile = join(repo, "pids.json");
    for (const external of externalRequests) {
      rmSync(pidFile, { force: true });
      const executor = new BridgeExecutor({
        bin: fakeBridge,
        cwd: repo,
        env: { FAKE_BRIDGE_MODE: "hang", FAKE_PID_FILE: pidFile, FAKE_CHILDREN: "2" },
        externalAgentTimeoutMs: 1_500,
      });
      const started = Date.now();
      const pending = executor.execute(external);
      const children = await waitFor(() =>
        existsSync(pidFile) ? (JSON.parse(readFileSync(pidFile, "utf8")) as number[]) : undefined,
      );
      const completion = await pending;
      expect(Date.now() - started).toBeLessThan(10_000);
      expect(completion).toEqual({
        protocolVersion: PROTOCOL_VERSION,
        commandId: "fix-tests",
        status: "unknown",
        error: {
          message: "bridge timed out after 1500ms",
          details: { phase: "execute", timedOut: true, repositoryMayBeModified: true },
        },
      });
      await waitFor(() => (children.some(isAlive) ? undefined : true));
    }
  });

  it("refuses an invalid external agent timeout without starting anything", async () => {
    for (const externalAgentTimeoutMs of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, 2 ** 31]) {
      const completion = await new BridgeExecutor({
        bin: fakeBridge,
        cwd: repo,
        externalAgentTimeoutMs,
      }).execute(externalRequests[0]!);
      expect(completion.status, String(externalAgentTimeoutMs)).toBe("failed");
      expect(completion.error?.details).toEqual({
        phase: "config",
        repositoryMayBeModified: false,
      });
    }
    expect(DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS).toBe(30 * 60 * 1000);
    // The builtin backend has no host timeout.
    expect(
      (
        await new BridgeExecutor({ bin: fakeBridge, cwd: repo, externalAgentTimeoutMs: 0 }).execute(
          request,
        )
      ).status,
    ).toBe("succeeded");
  });

  it("records the backend in history, replays it offline, and treats a backend change as changed", async () => {
    const executor = new BridgeExecutor({ bin: fakeBridge, cwd: repo });
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

    const offline = new BridgeExecutor({ bin: join(repo, "missing-bridge"), cwd: repo });
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

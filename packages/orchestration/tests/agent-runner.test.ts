/**
 * `agent` steps on the node and quickjs runtimes (`execution/agent.ts`):
 * one `codemod-agent` process per step, started through the host, here
 * against a stand-in runner (`fixtures/fake-agent.mjs`). The runner itself
 * is tested in `crates/orchestration-agent`; the QuickJS host path in
 * `quickjs.e2e.test.ts`.
 */
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { getHost } from "../src/core/host.ts";
import {
  DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS,
  executeAgent,
  type AgentExecutionOptions,
} from "../src/execution/agent.ts";
import { canListProcesses } from "../src/execution/process-tree.ts";
import {
  MemoryHistoryStore,
  PROTOCOL_VERSION,
  agent,
  dynamic,
  run,
  type OperationRequest,
} from "../src/index.ts";

const fakeAgent = resolve(import.meta.dirname, "fixtures/fake-agent.mjs");

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

type Echo = {
  request: OperationRequest;
  cwd: string;
  env: Record<string, string>;
  secrets: unknown;
};
const echoed = (completion: { output?: unknown }) =>
  JSON.parse((completion.output as { text: string }).text) as Echo;

describe("agent steps through the codemod-agent runner", () => {
  let repo: string;
  const options = (extra: Partial<AgentExecutionOptions> = {}): AgentExecutionOptions => ({
    bin: fakeAgent,
    cwd: repo,
    hostEnv: { PATH: process.env.PATH },
    ...extra,
  });
  beforeEach(() => {
    repo = mkdtempSync(join(tmpdir(), "agent-runner-"));
  });
  afterEach(() => rmSync(repo, { recursive: true, force: true }));

  it("starts the runner in the target with the allowlisted environment and the key on stdin", async () => {
    const hostEnv = {
      PATH: process.env.PATH,
      HOME: "/home/dev",
      LC_ALL: "C",
      LLM_API_KEY: "llm-key",
      LLM_MODEL: "gpt-4o",
      TYPESAFE_API_KEY: "typesafe-key",
      AWS_SECRET_ACCESS_KEY: "aws",
      GITHUB_TOKEN: "gh",
    };
    const completion = await executeAgent(options({ hostEnv, env: { EXTRA: "1" } }), request);
    expect(completion.status, JSON.stringify(completion)).toBe("succeeded");
    const { env, secrets, cwd, request: sent } = echoed(completion);
    expect(sent).toEqual(request);
    expect(realpathSync(cwd)).toBe(realpathSync(repo));
    expect(env).toMatchObject({ EXTRA: "1", HOME: "/home/dev", CODEMOD_AGENT_SECRETS: "stdin" });
    // The key reaches the runner on stdin, never in its launch environment.
    expect(env).not.toHaveProperty("LLM_API_KEY");
    expect(secrets).toEqual({ LLM_API_KEY: "llm-key" });
    for (const secret of ["TYPESAFE_API_KEY", "AWS_SECRET_ACCESS_KEY", "GITHUB_TOKEN"]) {
      expect(env).not.toHaveProperty(secret);
    }
    // Without a key there is no secrets line and no marker.
    const keyless = echoed(await executeAgent(options(), request));
    expect(keyless.secrets).toBeNull();
    expect(keyless.env).not.toHaveProperty("CODEMOD_AGENT_SECRETS");
  });

  it("gives external backends no LLM settings or secret, and refuses explicit credentials", async () => {
    const hostEnv = {
      PATH: process.env.PATH,
      HOME: "/home/dev",
      CODEX_HOME: "/home/dev/.codex-work",
      LLM_API_KEY: "llm-key",
      LLM_PROVIDER: "openai",
      OPENAI_API_KEY: "openai",
    };
    for (const external of externalRequests) {
      const { env, secrets } = echoed(await executeAgent(options({ hostEnv }), external));
      expect(secrets).toBeNull();
      expect(env).toMatchObject({ HOME: "/home/dev", CODEX_HOME: "/home/dev/.codex-work" });
      for (const name of [
        "LLM_API_KEY",
        "LLM_PROVIDER",
        "OPENAI_API_KEY",
        "CODEMOD_AGENT_SECRETS",
      ]) {
        expect(env).not.toHaveProperty(name);
      }
      const refused = await executeAgent(
        options({ env: { OPENAI_API_KEY: "sk-test", EXTRA: "1" } }),
        external,
      );
      expect(refused).toEqual({
        protocolVersion: PROTOCOL_VERSION,
        commandId: "fix-tests",
        status: "failed",
        error: {
          message: expect.stringContaining("(OPENAI_API_KEY)"),
          details: { phase: "config", repositoryMayBeModified: false },
        },
      });
      expect(JSON.stringify(refused)).not.toContain("sk-test");
    }
  });

  it("refuses before starting anything: no runner, bad timeout, abort", async () => {
    for (const bin of [undefined, join(repo, "missing-agent"), repo]) {
      const completion = await executeAgent(options({ bin }), request);
      expect(completion).toMatchObject({
        status: "failed",
        error: {
          message: expect.stringContaining("cargo build -p orchestration-agent"),
          details: { phase: "config", repositoryMayBeModified: false },
        },
      });
    }
    for (const externalAgentTimeoutMs of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, 2 ** 31]) {
      const completion = await executeAgent(
        options({ externalAgentTimeoutMs }),
        externalRequests[0]!,
      );
      expect(completion.error?.details, String(externalAgentTimeoutMs)).toEqual({
        phase: "config",
        repositoryMayBeModified: false,
      });
    }
    // The builtin backend has no host timeout.
    expect((await executeAgent(options({ externalAgentTimeoutMs: 0 }), request)).status).toBe(
      "succeeded",
    );
    expect(DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS).toBe(30 * 60 * 1000);
    for (const r of [request, ...externalRequests]) {
      expect(await executeAgent(options(), r, AbortSignal.abort())).toMatchObject({
        status: "cancelled",
        error: { details: { phase: "start", repositoryMayBeModified: false } },
      });
    }
  });

  it.each([
    ["builtin", request, "0"],
    ["claude-code", externalRequests[0]!, "0"],
    ["codex", externalRequests[1]!, "1"],
  ] as const)(
    "is unknown when a %s agent is aborted after spawn, and kills the runner's process tree",
    async (_backend, r, detach) => {
      if (detach === "1" && (process.platform === "win32" || !canListProcesses())) return;
      const pidFile = join(repo, "pids.json");
      const controller = new AbortController();
      const pending = executeAgent(
        options({
          env: {
            FAKE_AGENT_MODE: "hang",
            FAKE_PID_FILE: pidFile,
            FAKE_CHILDREN: "2",
            FAKE_DETACH_CHILDREN: detach,
          },
        }),
        r,
        controller.signal,
      );
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

  it("times external agents out on the host clock, killing the process tree", async () => {
    const pidFile = join(repo, "pids.json");
    const pending = executeAgent(
      options({
        env: { FAKE_AGENT_MODE: "hang", FAKE_PID_FILE: pidFile, FAKE_CHILDREN: "2" },
        externalAgentTimeoutMs: 1_500,
      }),
      externalRequests[1]!,
    );
    const children = await waitFor(() =>
      existsSync(pidFile) ? (JSON.parse(readFileSync(pidFile, "utf8")) as number[]) : undefined,
    );
    expect(await pending).toEqual({
      protocolVersion: PROTOCOL_VERSION,
      commandId: "fix-tests",
      status: "unknown",
      error: {
        message: "agent timed out after 1500ms",
        details: { phase: "execute", timedOut: true, repositoryMayBeModified: true },
      },
    });
    await waitFor(() => (children.some(isAlive) ? undefined : true));
  });

  it.each([
    ["garbage", /invalid|JSON/u, "runner"],
    ["other-command", /completion for 'other' while running 'fix-tests'/u, "runner"],
    ["silent", /exited with code 1 and wrote no completion/u, "execute"],
  ])(
    "treats a %s runner as unknown, since the agent may have changed files",
    async (mode, message, phase) => {
      const completion = await executeAgent(options({ env: { FAKE_AGENT_MODE: mode } }), request);
      expect(completion).toMatchObject({
        status: "unknown",
        error: {
          message: expect.stringMatching(message),
          details: { phase, repositoryMayBeModified: true },
        },
      });
    },
  );

  it("records the agent's completion and replays it without a runner", async () => {
    const fix = agent({
      name: "fix-tests",
      prompt: "Fix it.",
      backend: { kind: "builtin", tools: [] },
    });
    const workflow = dynamic(async () => JSON.parse((await fix()).text).cwd as string);
    const store = new MemoryHistoryStore();
    const executor = {
      execute: (r: OperationRequest, signal?: AbortSignal) => executeAgent(options(), r, signal),
    };
    const first = await run(workflow, { executor, history: store });
    expect(realpathSync(first.output)).toBe(realpathSync(repo));
    const offline = {
      execute: (r: OperationRequest) => executeAgent(options({ bin: undefined }), r),
    };
    const replay = await run(workflow, { executor: offline, history: store });
    expect(replay.replayed).toBe(true);
    expect(replay.output).toBe(first.output);
    expect(getHost().name).toBe("node");
  });
});

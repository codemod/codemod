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
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { executeAgent, type AgentExecutionOptions } from "../src/execution/agent.ts";
import { canListProcesses } from "../src/execution/process-tree.ts";
import {
  MemoryHistoryStore,
  PROTOCOL_VERSION,
  agent,
  dynamic,
  run,
  type AgentBackend,
  type OperationCompletion,
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

/** The pids still alive once they had up to five seconds to exit. */
const survivors = (pids: number[]) =>
  waitFor(() => (pids.some(isAlive) ? undefined : [])).catch(() => pids.filter(isAlive));

const requestFor = (backend: AgentBackend): OperationRequest => ({
  protocolVersion: PROTOCOL_VERSION,
  commandId: "fix-tests",
  operation: { kind: "agent", prompt: "x", backend },
});
const builtin: AgentBackend = { kind: "builtin", tools: [] };
const request = requestFor(builtin);
const backends: Record<string, OperationRequest> = {
  builtin: request,
  "claude-code": requestFor({ kind: "claude-code", tools: [] }),
  codex: requestFor({ kind: "codex", sandbox: "read-only" }),
};
const external = ["claude-code", "codex"];

type Echo = { request: unknown; cwd: string; env: Record<string, string>; secrets: unknown };
const echoed = (completion: { output?: unknown }) =>
  JSON.parse((completion.output as { text: string }).text) as Echo;

/** The given variables that are set in `env`, with their values. */
const pick = (env: Record<string, string>, names: string[]) =>
  Object.fromEntries(names.filter((name) => name in env).map((name) => [name, env[name]]));

const hostEnv = {
  PATH: process.env.PATH,
  HOME: "/home/dev",
  CODEX_HOME: "/home/dev/.codex-work",
  LC_ALL: "C",
  LLM_API_KEY: "llm-key",
  LLM_MODEL: "gpt-4o",
  LLM_PROVIDER: "openai",
  OPENAI_API_KEY: "openai",
  DECISION_API_KEY: "decision-key",
  AWS_SECRET_ACCESS_KEY: "aws",
  GITHUB_TOKEN: "gh",
};
const credentials = ["LLM_API_KEY", "DECISION_API_KEY", "AWS_SECRET_ACCESS_KEY", "GITHUB_TOKEN"];
const configFailure = { phase: "config", repositoryMayBeModified: false };
const mayHaveChangedFiles = { phase: "execute", repositoryMayBeModified: true };
const unknownAfterStart = (message: RegExp, phase = "execute") => ({
  status: "unknown",
  error: {
    message: expect.stringMatching(message),
    details: { phase, repositoryMayBeModified: true },
  },
});

const repo = mkdtempSync(join(tmpdir(), "agent-runner-"));
afterAll(() => rmSync(repo, { recursive: true, force: true }));
const options = (extra: Partial<AgentExecutionOptions> = {}): AgentExecutionOptions => ({
  bin: fakeAgent,
  cwd: repo,
  hostEnv: { PATH: process.env.PATH },
  ...extra,
});

let hangs = 0;
/** Starts a runner that hangs with two children, then waits for its completion. */
async function hanging(backend: string, extra: Partial<AgentExecutionOptions>, abort = false) {
  const controller = new AbortController();
  const pidFile = join(repo, `${backend}-${++hangs}.pids.json`);
  const env = { FAKE_AGENT_MODE: "hang", FAKE_PID_FILE: pidFile, FAKE_CHILDREN: "2", ...extra.env };
  const pending = executeAgent(options({ ...extra, env }), backends[backend]!, controller.signal);
  const children = await waitFor(() =>
    existsSync(pidFile) ? (JSON.parse(readFileSync(pidFile, "utf8")) as number[]) : undefined,
  );
  const aliveAtStart = children.every(isAlive);
  if (abort) controller.abort();
  return { completion: await pending, children, aliveAtStart };
}

describe("a builtin agent", () => {
  let completion: OperationCompletion;
  let keyless: Echo;
  beforeAll(async () => {
    // An invalid external timeout: builtin agents are not bounded by it.
    const env = { EXTRA: "1" };
    completion = await executeAgent(options({ hostEnv, env, externalAgentTimeoutMs: 0 }), request);
    keyless = echoed(await executeAgent(options(), request));
  });

  it("ignores the external agent timeout", () => {
    expect(completion.status).toBe("succeeded");
  });

  it("hands the runner the request unchanged", () => {
    expect(echoed(completion).request).toEqual(request);
  });

  it("starts the runner in the target repository", () => {
    expect(realpathSync(echoed(completion).cwd)).toBe(realpathSync(repo));
  });

  it("starts the runner with allowlisted host variables and the explicit env, but no credentials", () => {
    expect(pick(echoed(completion).env, ["HOME", "EXTRA", ...credentials])).toEqual({
      HOME: "/home/dev",
      EXTRA: "1",
    });
  });

  it("sends the LLM key on stdin and marks that it did", () => {
    const { secrets, env } = echoed(completion);
    expect([secrets, env.CODEMOD_AGENT_SECRETS]).toEqual([{ LLM_API_KEY: "llm-key" }, "stdin"]);
  });

  it("sends no secrets and no marker without an LLM key", () => {
    expect([keyless.secrets, "CODEMOD_AGENT_SECRETS" in keyless.env]).toEqual([null, false]);
  });
});

describe.each(external)("a %s agent", (backend) => {
  let launched: Echo;
  beforeAll(async () => {
    launched = echoed(await executeAgent(options({ hostEnv }), backends[backend]!));
  });
  const refusal = () =>
    executeAgent(options({ env: { OPENAI_API_KEY: "sk-test", EXTRA: "1" } }), backends[backend]!);

  it("gets HOME and CODEX_HOME but no LLM settings, provider key or secrets", () => {
    const names = ["HOME", "CODEX_HOME", "LLM_PROVIDER", "OPENAI_API_KEY", "CODEMOD_AGENT_SECRETS"];
    expect([pick(launched.env, [...names, ...credentials]), launched.secrets]).toEqual([
      { HOME: "/home/dev", CODEX_HOME: "/home/dev/.codex-work" },
      null,
    ]);
  });

  it("is refused an explicit provider key before start", async () => {
    expect(await refusal()).toMatchObject({
      status: "failed",
      error: { message: expect.stringContaining("(OPENAI_API_KEY)"), details: configFailure },
    });
  });

  it("never repeats the refused key", async () => {
    expect(JSON.stringify(await refusal())).not.toContain("sk-test");
  });
});

it.each([
  ["no runner was found", undefined],
  ["the runner is missing", "missing-agent"],
  ["the runner is a directory", "."],
])("fails with build instructions when %s", async (_label, bin) => {
  const completion = await executeAgent(options({ bin: bin && join(repo, bin) }), request);
  expect(completion).toMatchObject({
    status: "failed",
    error: {
      message: expect.stringContaining("cargo build -p orchestration-agent"),
      details: configFailure,
    },
  });
});

it.each([0, -1, Number.NaN, Number.POSITIVE_INFINITY, 2 ** 31])(
  "rejects an external agent timeout of %s",
  async (externalAgentTimeoutMs) => {
    const completion = await executeAgent(options({ externalAgentTimeoutMs }), backends.codex!);
    expect(completion.error?.details).toEqual(configFailure);
  },
);

it.each(Object.keys(backends))("cancels a %s agent aborted before start", async (backend) => {
  expect(await executeAgent(options(), backends[backend]!, AbortSignal.abort())).toMatchObject({
    status: "cancelled",
    error: { details: { phase: "start", repositoryMayBeModified: false } },
  });
});

describe("an agent aborted after spawn", () => {
  // Detached children are found by listing processes, which some platforms cannot do.
  const detachable = process.platform !== "win32" && canListProcesses();
  const cases = Object.keys(backends).filter((backend) => backend !== "codex" || detachable);
  const aborted = new Map<string, Awaited<ReturnType<typeof hanging>>>();
  beforeAll(async () => {
    await Promise.all(
      cases.map(async (backend) => {
        // The codex case starts children in their own process groups.
        const env = { FAKE_DETACH_CHILDREN: backend === "codex" ? "1" : "0" };
        aborted.set(backend, await hanging(backend, { env }, true));
      }),
    );
  });

  it.each(cases)("reports a %s agent as unknown, since it may have changed files", (backend) => {
    const message = /killed by SIGKILL on abort|aborted while/u;
    expect(aborted.get(backend)!.completion).toMatchObject(unknownAfterStart(message));
  });

  it.each(cases)("kills a %s agent's process tree", async (backend) => {
    const { aliveAtStart, children } = aborted.get(backend)!;
    expect([aliveAtStart, await survivors(children)]).toEqual([true, []]);
  });
});

describe("an external agent past its timeout", () => {
  let timedOut: Awaited<ReturnType<typeof hanging>>;
  beforeAll(async () => {
    timedOut = await hanging("codex", { externalAgentTimeoutMs: 1_500 });
  });

  it("is unknown and marked timed out on the host clock", () => {
    expect(timedOut.completion).toMatchObject({
      status: "unknown",
      error: {
        message: "agent timed out after 1500ms",
        details: { ...mayHaveChangedFiles, timedOut: true },
      },
    });
  });

  it("has its process tree killed", async () => {
    expect([timedOut.aliveAtStart, await survivors(timedOut.children)]).toEqual([true, []]);
  });
});

it.each([
  ["writes garbage", "garbage", /invalid|JSON/u, "runner"],
  ["answers for another command", "other-command", /for 'other'/u, "runner"],
  ["exits without a completion", "silent", /wrote no completion/u, "execute"],
])(
  "treats a runner that %s as unknown, since the agent may have changed files",
  async (_label, mode, message, phase) => {
    const completion = await executeAgent(options({ env: { FAKE_AGENT_MODE: mode } }), request);
    expect(completion).toMatchObject(unknownAfterStart(message, phase));
  },
);

it("replays a recorded agent step without a runner", async () => {
  const fix = agent({ name: "fix-tests", prompt: "Fix it.", backend: builtin });
  const workflow = dynamic(async () => JSON.parse((await fix()).text).cwd as string);
  const history = new MemoryHistoryStore();
  const runner = (bin?: string) => ({
    execute: (r: OperationRequest, signal?: AbortSignal) =>
      executeAgent(options({ bin }), r, signal),
  });
  await run(workflow, { executor: runner(fakeAgent), history });
  const replay = await run(workflow, { executor: runner(undefined), history });
  expect([replay.replayed, realpathSync(replay.output)]).toEqual([true, realpathSync(repo)]);
});

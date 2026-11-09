/**
 * `agent` operations: one `codemod-agent` process per operation
 * (`crates/orchestration-agent`), started through the host, so the same
 * code runs inside the QuickJS host and in the Node test harness.
 *
 * An agent runs out of process because its tools (the builtin `bash`, file
 * tools, MCP servers, the `claude`/`codex` CLIs) run in the process working
 * directory and inherit its environment. The runner starts in the target
 * root with the allowlisted environment of `agent-env.ts` (no host
 * credentials; external backends get no provider variables at all), and the
 * builtin agent's `LLM_API_KEY` travels on stdin after the request, never in
 * the environment. The completion comes back on the runner's stdout, which
 * the runner moves out of reach of everything it starts.
 *
 * Outcomes: a refusal before start (bad settings, a credential passed to an
 * external backend, no runner binary, an abort before spawn) is `failed` /
 * `cancelled` with `repositoryMayBeModified: false`. Once the runner exists,
 * its tools may already have edited files: an abort, the external timeout
 * (`timedOut`), a crash, or a missing or mismatched completion is `unknown`
 * with `repositoryMayBeModified: true`. Aborting kills the runner's process
 * tree (`Host.spawn`).
 *
 * Portable (`tests/portable.test.ts`): nothing here imports Node.
 */
import { getHost, type Host } from "../core/host.ts";
import type { Json } from "../core/json.ts";
import {
  PROTOCOL_VERSION,
  parseCompletion,
  type AgentOperation,
  type OperationCompletion,
  type OperationRequest,
} from "../core/protocol.ts";
import { AGENT_SECRETS_ENV, agentLaunch, externalAgentEnvProblem } from "./agent-env.ts";

/** The runner binary's file name. */
export const AGENT_BINARY = "codemod-agent";
/** Overrides where the runtimes look for the runner. */
export const AGENT_BIN_ENV = "CODEMOD_AGENT_BIN";
/** Default wall-clock limit for one `claude-code` or `codex` agent step: 30 minutes. */
export const DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS = 30 * 60 * 1000;
/** Largest delay a host timer honors. */
const MAX_TIMER_MS = 2_147_483_647;

export interface AgentExecutionOptions {
  /** The `codemod-agent` binary; `undefined` when none was found (a config failure). */
  bin: string | undefined;
  /** Repository root: the runner's working directory. */
  cwd: string;
  /** Explicit environment for the runner, on top of the allowlist (`agentLaunch`). */
  env?: Record<string, string>;
  /** Environment the allowlist reads from. Default: the host's. */
  hostEnv?: Readonly<Record<string, string | undefined>>;
  /**
   * Wall-clock limit for one external (`claude-code` or `codex`) agent, in
   * milliseconds. Default `DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS`. Builtin agents
   * are bounded by `maxSteps` instead. Host configuration, never history.
   */
  externalAgentTimeoutMs?: number;
  /** Default: the installed host. */
  host?: Host;
}

export async function executeAgent(
  options: AgentExecutionOptions,
  request: OperationRequest,
  signal?: AbortSignal,
): Promise<OperationCompletion> {
  const operation = request.operation as AgentOperation;
  const host = options.host ?? getHost();
  const completion = (
    status: "failed" | "cancelled" | "unknown",
    message: string,
    details: { [key: string]: Json },
  ): OperationCompletion => ({
    protocolVersion: PROTOCOL_VERSION,
    commandId: request.commandId,
    status,
    error: { message, details },
  });
  const refuse = (message: string) =>
    completion("failed", message, { phase: "config", repositoryMayBeModified: false });
  const afterStart = (message: string, details: { [key: string]: Json } = {}) =>
    completion("unknown", message, { phase: "execute", ...details, repositoryMayBeModified: true });

  if (operation.kind !== "agent") return refuse(`not an agent operation: ${operation.kind}`);
  const external = operation.backend.kind !== "builtin";
  const timeoutMs = options.externalAgentTimeoutMs ?? DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS;
  if (external) {
    if (!Number.isFinite(timeoutMs) || timeoutMs < 1 || timeoutMs > MAX_TIMER_MS) {
      return refuse(`externalAgentTimeoutMs must be a finite number from 1 to ${MAX_TIMER_MS}`);
    }
    const problem = externalAgentEnvProblem(options.env);
    if (problem !== undefined) return refuse(problem);
  }
  if (options.bin === undefined || !isFile(host, options.bin)) {
    return refuse(
      `agent steps need the ${AGENT_BINARY} runner${options.bin === undefined ? "" : `, not found at ${options.bin}`}; build it with 'cargo build -p orchestration-agent' or set ${AGENT_BIN_ENV}`,
    );
  }
  if (signal?.aborted) {
    return completion("cancelled", "aborted before start", {
      phase: "start",
      repositoryMayBeModified: false,
    });
  }

  const launch = agentLaunch(
    { ...(options.hostEnv ?? host.env()) },
    options.env,
    host.os.platform,
    operation.backend.kind,
    AGENT_SECRETS_ENV,
  );
  const stop = new AbortController();
  const onAbort = () => stop.abort();
  signal?.addEventListener("abort", onAbort, { once: true });
  let timedOut = false;
  const timer = external
    ? host.setTimeout(() => {
        timedOut = true;
        stop.abort();
      }, timeoutMs)
    : undefined;
  let result;
  try {
    const child = host.spawn({
      file: options.bin,
      args: [],
      cwd: options.cwd,
      env: launch.env,
      stdin: `${JSON.stringify(request)}\n${launch.stdin ?? ""}`,
      stderr: "ignore",
      signal: stop.signal,
    });
    result = await child.result;
  } catch (error) {
    return afterStart(`failed to start ${AGENT_BINARY}: ${(error as Error).message}`, {
      phase: "runner",
    });
  } finally {
    signal?.removeEventListener("abort", onAbort);
    if (timer !== undefined) host.clearTimeout(timer);
  }

  if (timedOut) {
    return afterStart(`agent timed out after ${timeoutMs}ms`, { timedOut: true });
  }
  if (signal?.aborted) {
    return afterStart(
      result.signal === null
        ? "aborted while the agent was finishing"
        : `agent killed by ${result.signal} on abort`,
    );
  }
  if (result.stdout === "") {
    return afterStart(
      result.signal === null
        ? `${AGENT_BINARY} exited with code ${result.code} and wrote no completion`
        : `${AGENT_BINARY} killed by ${result.signal}`,
    );
  }
  try {
    const parsed = parseCompletion(result.stdout);
    if (parsed.commandId !== request.commandId) {
      return afterStart(
        `${AGENT_BINARY} returned a completion for '${parsed.commandId}' while running '${request.commandId}'`,
        { phase: "runner" },
      );
    }
    return parsed;
  } catch (error) {
    return afterStart((error as Error).message, { phase: "runner" });
  }
}

function isFile(host: Host, path: string): boolean {
  try {
    const stat = host.fs.lstat(path);
    // A symlink to the binary (an installed shim) is fine: realpath must be a file.
    return stat.isFile || (stat.isSymbolicLink && host.fs.lstat(host.fs.realpath(path)).isFile);
  } catch {
    return false;
  }
}

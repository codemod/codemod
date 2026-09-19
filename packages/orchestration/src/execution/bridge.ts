/**
 * One `butterflow-execution-bridge <request.json> <response.json>` process
 * per request. The exchange files live in a private directory under a
 * host-owned per-user root (`exchange.ts`), outside the target and outside
 * anything a sandboxed agent may write; workflow code never sees them. The
 * response is created exclusively by the bridge and read without following
 * symlinks. Aborting the
 * signal kills the bridge and, best effort, every process it started
 * (`process-tree.ts`); the host's `exit` does the same for bridges still running.
 *
 * Outcome after an abort or a kill: `shell` and `jssg` keep `cancelled` when the
 * bridge died by the signal. An `agent` that had been spawned is `unknown`
 * with `details.repositoryMayBeModified`: its tools may already have edited
 * files, so only an abort before spawn is `cancelled`.
 *
 * `timeoutMs` is a host-owned wall-clock limit: when it passes, the bridge's
 * process tree is killed exactly as on abort and the completion is `unknown`
 * with `details.timedOut` (and, for an agent, `repositoryMayBeModified`).
 */
import { spawn } from "node:child_process";
import { nullSink, type EventSink } from "../core/events.ts";
import type { Json } from "../core/json.ts";
import { createExchange, readResponse, resolveExchangeRoot } from "./exchange.ts";
import { killProcessTree, spawnOptions, trackProcessTree } from "./process-tree.ts";
import {
  PROTOCOL_VERSION,
  parseCompletion,
  type OperationCompletion,
  type OperationRequest,
} from "../core/protocol.ts";

export interface BridgeProcessOptions {
  /** Absolute path to the bridge binary. */
  bin: string;
  /** Working directory of the bridge process (where `shell` runs). */
  cwd: string;
  /** Added to the inherited environment, or the whole environment when `inheritEnv` is false. */
  env?: Record<string, string>;
  /** Default true. False starts the bridge with `env` only (see `agentLaunch`). */
  inheritEnv?: boolean;
  /** Written to the bridge's stdin, then closed; stdin is ignored without it. Never logged. */
  stdin?: string;
  /** Wall-clock limit for the bridge process; unset means none. Must be a positive finite number. */
  timeoutMs?: number;
  events?: EventSink;
}

/**
 * Run one request through the bridge binary. Never throws: spawn failures,
 * missing responses, and aborts are reported as completions. `unknown` means
 * the bridge may have acted before the host lost track of it.
 */
export async function spawnBridge(
  options: BridgeProcessOptions,
  request: OperationRequest,
  signal?: AbortSignal,
): Promise<OperationCompletion> {
  const agent = request.operation.kind === "agent";
  const completion = (
    status: "failed" | "cancelled" | "unknown",
    message: string,
    details?: { [key: string]: Json },
  ): OperationCompletion => ({
    protocolVersion: PROTOCOL_VERSION,
    commandId: request.commandId,
    status,
    error: details === undefined ? { message } : { message, details },
  });
  /** An agent may have changed files once its bridge exists. */
  const afterStart = (
    status: "cancelled" | "unknown",
    message: string,
    phase: "execute" | "bridge",
  ): OperationCompletion =>
    agent
      ? completion("unknown", message, { phase, repositoryMayBeModified: true })
      : completion(status, message);
  if (signal?.aborted) {
    return agent
      ? completion("cancelled", "aborted before start", {
          phase: "start",
          repositoryMayBeModified: false,
        })
      : completion("cancelled", "aborted before start");
  }
  const notStarted = (message: string): OperationCompletion =>
    agent
      ? completion("failed", message, { phase: "config", repositoryMayBeModified: false })
      : completion("failed", message);
  const located = resolveExchangeRoot(options.cwd);
  if ("problem" in located) return notStarted(located.problem);
  let exchange;
  try {
    exchange = createExchange(located.root, JSON.stringify(request));
  } catch (error) {
    return notStarted(`failed to create bridge exchange: ${(error as Error).message}`);
  }
  const { requestPath, responsePath } = exchange;
  let timedOut = false;
  try {
    const exit = await new Promise<{ code: number | null; signal: NodeJS.Signals | null }>(
      (resolve, reject) => {
        const child = spawn(options.bin, [requestPath, responsePath], {
          ...spawnOptions(),
          cwd: options.cwd,
          env:
            options.inheritEnv === false ? { ...options.env } : { ...process.env, ...options.env },
          stdio: [options.stdin === undefined ? "ignore" : "pipe", "ignore", "ignore"],
        });
        if (options.stdin !== undefined && child.stdin !== null) {
          // A bridge that exits early closes the pipe; that is not a host error.
          child.stdin.on("error", () => {});
          child.stdin.end(options.stdin);
        }
        const untrack = trackProcessTree(child.pid);
        (options.events ?? nullSink).emit({
          type: "bridge.spawned",
          commandId: request.commandId,
          pid: child.pid,
        });
        const onAbort = () => killProcessTree(child.pid);
        signal?.addEventListener("abort", onAbort, { once: true });
        const timer =
          options.timeoutMs === undefined
            ? undefined
            : setTimeout(() => {
                timedOut = true;
                killProcessTree(child.pid);
              }, options.timeoutMs);
        const settle = () => {
          signal?.removeEventListener("abort", onAbort);
          if (timer !== undefined) clearTimeout(timer);
          untrack();
        };
        child.once("error", (error) => {
          settle();
          reject(error);
        });
        child.once("close", (code, exitSignal) => {
          settle();
          resolve({ code, signal: exitSignal });
        });
      },
    ).catch((error: Error) => ({ code: null, signal: null, spawnError: error }));
    if ("spawnError" in exit) {
      return afterStart("unknown", `failed to start bridge: ${exit.spawnError.message}`, "bridge");
    }
    if (timedOut) {
      return completion("unknown", `bridge timed out after ${options.timeoutMs}ms`, {
        phase: "execute",
        timedOut: true,
        ...(agent ? { repositoryMayBeModified: true } : {}),
      });
    }
    if (signal?.aborted) {
      // The command may have run to completion or been killed part-way;
      // the bridge cannot tell us which side effects happened.
      return exit.signal
        ? afterStart("cancelled", `bridge killed by ${exit.signal} on abort`, "execute")
        : afterStart("unknown", "aborted while the command was finishing", "execute");
    }
    const read = readResponse(responsePath);
    if (read.kind === "rejected") {
      return afterStart("unknown", `bridge response rejected: ${read.reason}`, "bridge");
    }
    if (read.kind === "missing") {
      return exit.signal
        ? afterStart("cancelled", `bridge killed by ${exit.signal}`, "execute")
        : afterStart(
            "unknown",
            `bridge exited with code ${exit.code} and wrote no response`,
            "execute",
          );
    }
    try {
      const parsed = parseCompletion(read.text);
      if (parsed.commandId !== request.commandId) {
        return afterStart(
          "unknown",
          `bridge returned completion for '${parsed.commandId}' while running '${request.commandId}'`,
          "bridge",
        );
      }
      return parsed;
    } catch (error) {
      return afterStart("unknown", (error as Error).message, "bridge");
    }
  } finally {
    exchange.cleanup();
  }
}

/**
 * One `butterflow-execution-bridge <request.json> <response.json>` process
 * per request. The exchange files live in a private temp directory written
 * by this trusted host only; workflow code never sees them. Aborting the
 * signal kills the process with SIGKILL, so no bridge outlives its host.
 */
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { nullSink, type EventSink } from "./events.ts";
import {
  PROTOCOL_VERSION,
  parseCompletion,
  type OperationCompletion,
  type OperationRequest,
} from "./protocol.ts";

export interface BridgeProcessOptions {
  /** Absolute path to the bridge binary. */
  bin: string;
  /** Working directory of the bridge process (where `exec` runs). */
  cwd: string;
  env?: Record<string, string>;
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
  const completion = (status: "failed" | "cancelled" | "unknown", message: string) => ({
    protocolVersion: PROTOCOL_VERSION,
    commandId: request.commandId,
    status,
    error: { message },
  });
  if (signal?.aborted) return completion("cancelled", "aborted before start");
  const exchange = mkdtempSync(join(tmpdir(), "codemod-bridge-"));
  const requestPath = join(exchange, "request.json");
  const responsePath = join(exchange, "response.json");
  try {
    writeFileSync(requestPath, JSON.stringify(request));
    const exit = await new Promise<{ code: number | null; signal: NodeJS.Signals | null }>(
      (resolve, reject) => {
        const child = spawn(options.bin, [requestPath, responsePath], {
          cwd: options.cwd,
          env: { ...process.env, ...options.env },
          stdio: "ignore",
        });
        (options.events ?? nullSink).emit({
          type: "bridge.spawned",
          commandId: request.commandId,
          pid: child.pid,
        });
        const onAbort = () => child.kill("SIGKILL");
        signal?.addEventListener("abort", onAbort, { once: true });
        child.once("error", (error) => {
          signal?.removeEventListener("abort", onAbort);
          reject(error);
        });
        child.once("close", (code, exitSignal) => {
          signal?.removeEventListener("abort", onAbort);
          resolve({ code, signal: exitSignal });
        });
      },
    ).catch((error: Error) => ({ code: null, signal: null, spawnError: error }));
    if ("spawnError" in exit) {
      return completion("unknown", `failed to start bridge: ${exit.spawnError.message}`);
    }
    if (signal?.aborted) {
      // The command may have run to completion or been killed part-way;
      // the bridge cannot tell us which side effects happened.
      return exit.signal
        ? completion("cancelled", `bridge killed by ${exit.signal} on abort`)
        : completion("unknown", "aborted while the command was finishing");
    }
    let response: string;
    try {
      response = readFileSync(responsePath, "utf8");
    } catch {
      return exit.signal
        ? completion("cancelled", `bridge killed by ${exit.signal}`)
        : completion("unknown", `bridge exited with code ${exit.code} and wrote no response`);
    }
    try {
      const parsed = parseCompletion(response);
      if (parsed.commandId !== request.commandId) {
        return completion(
          "unknown",
          `bridge returned completion for '${parsed.commandId}' while running '${request.commandId}'`,
        );
      }
      return parsed;
    } catch (error) {
      return completion("unknown", (error as Error).message);
    }
  } finally {
    rmSync(exchange, { recursive: true, force: true });
  }
}

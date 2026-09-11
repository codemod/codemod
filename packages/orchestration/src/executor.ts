/**
 * Migration seam: how an `OperationRequest` becomes an `OperationCompletion`.
 * Implementations: `BridgeExecutor` (Rust bridge for `exec`, TypeScript JSSG
 * orchestration over the Rust JSSG worker) and the harness's scripted
 * executor. A future AI adapter plugs in here.
 */
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import type { EventSink } from "./events.ts";
import { executeJssg } from "./jssg.ts";
import {
  PROTOCOL_VERSION,
  parseCompletion,
  type OperationCompletion,
  type OperationRequest,
} from "./protocol.ts";

export interface OperationExecutor {
  /** `signal` aborts the operation; the completion is then `cancelled` or `unknown`. */
  execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion>;
}

export interface BridgeOptions {
  /** Path to the `butterflow-execution-bridge` binary (cargo build -p butterflow-execution-bridge). */
  bin: string;
  /** Working directory: where `exec` runs and the repository root for JSSG targets. */
  cwd?: string;
  /**
   * Directory that relative JSSG `script` paths resolve against, typically
   * the workflow file's directory. Sent to the worker's `open` message; it
   * never enters history. Defaults to `cwd`.
   */
  scriptRoot?: string;
  env?: Record<string, string>;
  /** Receives `jssg.worker` and `jssg.progress` events. */
  events?: EventSink;
}

/**
 * `exec`: one-shot file protocol (`butterflow-execution-bridge <request>
 * <response>`) through `butterflow_runners::DirectRunner`. `jssg`: the
 * TypeScript orchestrator in `jssg.ts` over one persistent worker process.
 * `ai`: refused. Only this trusted host process spawns anything; workflow
 * code never can.
 */
export class BridgeExecutor implements OperationExecutor {
  private readonly bin: string;
  private readonly cwd: string;
  private readonly scriptRoot: string;

  constructor(private readonly options: BridgeOptions) {
    this.bin = resolve(options.bin);
    this.cwd = resolve(options.cwd ?? process.cwd());
    this.scriptRoot = options.scriptRoot === undefined ? this.cwd : resolve(options.scriptRoot);
  }

  async execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion> {
    switch (request.operation.kind) {
      case "exec":
        return this.executeExec(request, signal);
      case "jssg":
        return executeJssg({
          bin: this.bin,
          cwd: this.cwd,
          scriptRoot: this.scriptRoot,
          commandId: request.commandId,
          operation: request.operation,
          signal,
          events: this.options.events,
          env: this.options.env,
        });
      case "ai":
        return completion(
          request,
          "failed",
          "operation kind 'ai' has no executor adapter in the execution bridge",
        );
    }
  }

  private async executeExec(
    request: OperationRequest,
    signal?: AbortSignal,
  ): Promise<OperationCompletion> {
    if (signal?.aborted) return completion(request, "cancelled", "aborted before start");
    const exchangeDir = mkdtempSync(join(tmpdir(), "codemod-bridge-"));
    const requestPath = join(exchangeDir, "request.json");
    const responsePath = join(exchangeDir, "response.json");
    try {
      // Written by the trusted host only; workflow code never sees these paths.
      writeFileSync(requestPath, JSON.stringify(request));
      let code: number | null;
      let exitSignal: NodeJS.Signals | null;
      try {
        ({ code, signal: exitSignal } = await runBridge(
          this.bin,
          { cwd: this.cwd, env: this.options.env },
          requestPath,
          responsePath,
          signal,
        ));
      } catch (error) {
        return completion(
          request,
          "unknown",
          `failed to start bridge: ${(error as Error).message}`,
        );
      }
      if (signal?.aborted) {
        // The command may have run to completion or been killed part-way;
        // the bridge cannot tell us which side effects happened.
        return completion(
          request,
          exitSignal ? "cancelled" : "unknown",
          exitSignal
            ? `bridge killed by ${exitSignal} on abort`
            : "aborted while the command was finishing",
        );
      }
      const response = readResponse(responsePath);
      if (response !== undefined) {
        try {
          const parsed = parseCompletion(response);
          if (parsed.commandId !== request.commandId) {
            return completion(
              request,
              "unknown",
              `bridge returned completion for '${parsed.commandId}' while running '${request.commandId}'`,
            );
          }
          return parsed;
        } catch (error) {
          return completion(request, "unknown", (error as Error).message);
        }
      }
      if (exitSignal) return completion(request, "cancelled", `bridge killed by ${exitSignal}`);
      return completion(
        request,
        "unknown",
        `bridge exited with code ${code} and wrote no response`,
      );
    } finally {
      rmSync(exchangeDir, { recursive: true, force: true });
    }
  }
}

function runBridge(
  bin: string,
  options: { cwd: string; env?: Record<string, string> },
  requestPath: string,
  responsePath: string,
  signal?: AbortSignal,
) {
  return new Promise<{ code: number | null; signal: NodeJS.Signals | null }>((resolve, reject) => {
    const child = spawn(bin, [requestPath, responsePath], {
      cwd: options.cwd,
      env: { ...process.env, ...options.env },
      stdio: "ignore",
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
  });
}

function readResponse(path: string): string | undefined {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return undefined;
  }
}

function completion(
  request: OperationRequest,
  status: "failed" | "cancelled" | "unknown",
  message: string,
): OperationCompletion {
  return {
    protocolVersion: PROTOCOL_VERSION,
    commandId: request.commandId,
    status,
    error: { message },
  };
}

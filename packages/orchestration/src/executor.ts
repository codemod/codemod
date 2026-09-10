/**
 * Migration seam: how an `OperationRequest` becomes an `OperationCompletion`.
 * Implementations: `BridgeExecutor` (Rust bridge over butterflow_runners) and
 * the harness's scripted executor. Future JSSG and AI adapters plug in here.
 */
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  PROTOCOL_VERSION,
  parseCompletion,
  type OperationCompletion,
  type OperationRequest,
} from "./protocol.ts";

export interface OperationExecutor {
  execute(request: OperationRequest): Promise<OperationCompletion>;
}

export interface BridgeOptions {
  /** Path to the `butterflow-execution-bridge` binary (cargo build -p butterflow-execution-bridge). */
  bin: string;
  /** Working directory for executed commands. */
  cwd?: string;
  env?: Record<string, string>;
}

/**
 * One-shot file protocol: writes the request JSON to a file, runs
 * `butterflow-execution-bridge <request> <response>`, and reads the completion
 * back. The Rust side calls `butterflow_runners::DirectRunner` and never
 * touches stdout/stderr. Only this trusted host process spawns anything or
 * touches the filesystem; workflow code never can.
 */
export class BridgeExecutor implements OperationExecutor {
  constructor(private readonly options: BridgeOptions) {}

  async execute(request: OperationRequest): Promise<OperationCompletion> {
    const exchangeDir = mkdtempSync(join(tmpdir(), "codemod-bridge-"));
    const requestPath = join(exchangeDir, "request.json");
    const responsePath = join(exchangeDir, "response.json");

    try {
      // Written by the trusted host only; workflow code never sees these paths.
      writeFileSync(requestPath, JSON.stringify(request));
      const { code, signal } = await runBridge(this.options, requestPath, responsePath);
      const response = readResponse(responsePath);
      if (response !== undefined) {
        try {
          return parseCompletion(response);
        } catch (error) {
          return completion(request, "unknown", (error as Error).message);
        }
      }
      if (signal) return completion(request, "cancelled", `bridge killed by ${signal}`);
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

function runBridge(options: BridgeOptions, requestPath: string, responsePath: string) {
  return new Promise<{ code: number | null; signal: NodeJS.Signals | null }>((resolve, reject) => {
    const child = spawn(options.bin, [requestPath, responsePath], {
      cwd: options.cwd,
      env: { ...process.env, ...options.env },
      stdio: "ignore",
    });
    child.once("error", reject);
    child.once("close", (code, signal) => resolve({ code, signal }));
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
  status: "cancelled" | "unknown",
  message: string,
): OperationCompletion {
  return {
    protocolVersion: PROTOCOL_VERSION,
    commandId: request.commandId,
    status,
    error: { message },
  };
}

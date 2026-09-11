/**
 * Migration seam: how an `OperationRequest` becomes an `OperationCompletion`.
 * Implementations: `BridgeExecutor` (the Rust bridge) and the harness's
 * scripted executor. A future AI adapter plugs in here.
 */
import { resolve } from "node:path";
import { spawnBridge } from "./bridge.ts";
import type { EventSink } from "./events.ts";
import { executeJssg } from "./jssg.ts";
import { PROTOCOL_VERSION, type OperationCompletion, type OperationRequest } from "./protocol.ts";

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
   * the workflow file's directory. Sent in the request context; it never
   * enters history. Defaults to `cwd`.
   */
  scriptRoot?: string;
  env?: Record<string, string>;
  /** Receives `bridge.spawned` events. */
  events?: EventSink;
}

/**
 * `exec`: one bridge process through `butterflow_runners::DirectRunner`.
 * `jssg`: the TypeScript orchestrator in `jssg.ts` around one bridge process.
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
    const { bin, cwd } = this;
    const { env, events } = this.options;
    switch (request.operation.kind) {
      case "exec":
        return spawnBridge({ bin, cwd, env, events }, request, signal);
      case "jssg":
        return executeJssg({
          bin,
          cwd,
          scriptRoot: this.scriptRoot,
          commandId: request.commandId,
          operation: request.operation,
          signal,
          events,
          env,
        });
      case "ai":
        return {
          protocolVersion: PROTOCOL_VERSION,
          commandId: request.commandId,
          status: "failed",
          error: { message: "operation kind 'ai' has no executor adapter in the execution bridge" },
        };
    }
  }
}

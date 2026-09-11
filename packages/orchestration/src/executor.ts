/**
 * Migration seam: how an `OperationRequest` becomes an `OperationCompletion`.
 * Implementations: `BridgeExecutor` (the Rust bridge) and the harness's
 * scripted executor. A future AI adapter plugs in here.
 */
import { resolve } from "node:path";
import { spawnBridge } from "./bridge.ts";
import type { ArtifactStore } from "./build.ts";
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
   * Built transform artifacts by hash, as `loadWorkflow()` collects them.
   * They are executor-side data: the source is sent in the request context
   * and never enters history. A JSSG command whose artifact is missing fails
   * before anything is spawned.
   */
  artifacts?: ArtifactStore;
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

  constructor(private readonly options: BridgeOptions) {
    this.bin = resolve(options.bin);
    this.cwd = resolve(options.cwd ?? process.cwd());
  }

  async execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion> {
    const { bin, cwd } = this;
    const { artifacts, env, events } = this.options;
    switch (request.operation.kind) {
      case "exec":
        return spawnBridge({ bin, cwd, env, events }, request, signal);
      case "jssg":
        return executeJssg({
          bin,
          cwd,
          artifacts,
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

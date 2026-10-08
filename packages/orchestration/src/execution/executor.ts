/**
 * Migration seam: how an `OperationRequest` becomes an `OperationCompletion`.
 * Implementations: `BridgeExecutor` (the Rust bridge) and the harness's
 * scripted executor.
 */
import { resolve } from "node:path";
import { agentLaunch, externalAgentEnvProblem } from "./agent-env.ts";
import { getAssessmentAsk } from "../core/assessment.ts";
import {
  executeAssessment,
  executeFileAssessment,
  type AssessmentExecutorOptions,
} from "./assessment.ts";
import { spawnBridge } from "./bridge.ts";
import type { ArtifactStore } from "../bundle/build.ts";
import type { EventSink } from "../core/events.ts";
import { executeJssg } from "./jssg.ts";
import {
  PROTOCOL_VERSION,
  type OperationCompletion,
  type OperationRequest,
} from "../core/protocol.ts";

/** Default wall-clock limit for one `claude-code` or `codex` agent step: 30 minutes. */
export const DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS = 30 * 60 * 1000;

/** Largest delay a Node timer honors. */
const MAX_TIMER_MS = 2_147_483_647;

export interface OperationExecutor {
  /** `signal` aborts the operation; the completion is then `cancelled` or `unknown`. */
  execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion>;
}

export interface BridgeOptions {
  /** Path to the `butterflow-execution-bridge` binary (cargo build -p butterflow-execution-bridge). */
  bin: string;
  /** Working directory: where `shell` runs and the repository root for JSSG targets. */
  cwd?: string;
  /**
   * Built transform artifacts by hash, as `loadWorkflow()` collects them.
   * They are executor-side data: the source is sent in the request context
   * and never enters history. A JSSG command whose artifact is missing fails
   * before anything is spawned.
   */
  artifacts?: ArtifactStore;
  /**
   * Extra variables for bridge processes. `shell` and `jssg` bridges inherit
   * the host environment plus these; an `agent` bridge gets only the
   * `agentEnvironment` allowlist plus these.
   */
  env?: Record<string, string>;
  /** Host environment the agent allowlist reads from. Default: `process.env`. */
  hostEnv?: Record<string, string | undefined>;
  /** Receives `bridge.spawned` events. */
  events?: EventSink;
  /**
   * Host wall-clock limit for one external (`claude-code` or `codex`) agent
   * bridge, in milliseconds. Default `DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS`
   * (30 minutes). When it passes the bridge's process tree is killed and the
   * command is `unknown` with `details.timedOut`. It is host configuration and
   * never part of history. `builtin` agents are bounded by `maxSteps` instead.
   */
  externalAgentTimeoutMs?: number;
  /**
   * TypeSafe SDK settings for `assessment`; unset fields fall back to the
   * SDK's `TYPESAFE_*` variables and defaults. Retries are the SDK's.
   */
  assessment?: AssessmentExecutorOptions;
}

/**
 * `shell`: one bridge process through `butterflow_runners::DirectRunner`.
 * `jssg`: the TypeScript orchestrator in `jssg.ts` around one bridge process.
 * `agent`: one bridge process in `cwd` with an allowlisted environment
 * (`agent-env.ts`) running the operation's backend. `builtin` runs
 * `codemod-ai` (`LLM_PROVIDER`, `LLM_MODEL`, `LLM_BASE_URL` in the
 * environment, `LLM_API_KEY` on stdin); `claude-code` and `codex` run the
 * installed CLI with its own login and no `LLM_*` variables.
 * `assessment`: one HTTP request from this process (`assessment.ts`); no
 * bridge, no repository access. Only this trusted host process spawns or
 * calls anything; workflow code never can.
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
      case "shell":
        return spawnBridge({ bin, cwd, env, events }, request, signal);
      case "agent": {
        const external = request.operation.backend.kind !== "builtin";
        const refuse = (message: string): OperationCompletion => ({
          protocolVersion: PROTOCOL_VERSION,
          commandId: request.commandId,
          status: "failed",
          error: { message, details: { phase: "config", repositoryMayBeModified: false } },
        });
        const timeoutMs = this.options.externalAgentTimeoutMs ?? DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS;
        if (external) {
          if (!Number.isFinite(timeoutMs) || timeoutMs < 1 || timeoutMs > MAX_TIMER_MS) {
            return refuse(
              `externalAgentTimeoutMs must be a finite number from 1 to ${MAX_TIMER_MS}`,
            );
          }
          const problem = externalAgentEnvProblem(env);
          if (problem !== undefined) return refuse(problem);
        }
        const launch = agentLaunch(
          this.options.hostEnv ?? process.env,
          env,
          process.platform,
          request.operation.backend.kind,
        );
        return spawnBridge(
          {
            bin,
            cwd,
            env: launch.env,
            inheritEnv: false,
            stdin: launch.stdin,
            events,
            ...(external ? { timeoutMs } : {}),
          },
          request,
          signal,
        );
      }
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
      case "assessment": {
        // File-oriented batch: __ask carries the per-file question resolver.
        const ask = getAssessmentAsk(request.operation);
        if (ask !== undefined) {
          return executeFileAssessment(
            this.options.assessment ?? {},
            this.cwd,
            request.commandId,
            request.operation,
            ask,
            signal,
          );
        }
        return executeAssessment(
          this.options.assessment ?? {},
          request.commandId,
          request.operation,
          signal,
        );
      }
    }
  }
}

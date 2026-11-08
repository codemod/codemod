/**
 * The executor of the `node` runtime (`node.ts`): `jssg` in-process over
 * the napi-rs ast-grep addon (`@codemod.com/jssg-napi`), through the
 * portable orchestrator (`execution/jssg.ts::executeJssgCommand` with the
 * portable batch loop of `execution/jssg-batch.ts` over the addon's
 * `JssgBatch`, and the addon's native file selection); `shell` through the
 * host (`execution/shell.ts::executeShell`); `assessment` through the
 * TypeSafe SDK in this process (`execution/assessment.ts`); `agent` in one
 * `codemod-agent` runner process each (`execution/agent.ts`).
 */
import type { ArtifactStore } from "../../bundle/build.ts";
import { executeAgent } from "../../execution/agent.ts";
import {
  executeAssessmentOperation,
  type AssessmentExecutorOptions,
} from "../../execution/assessment.ts";
import { executeJssgCommand, type JssgDispatch } from "../../execution/jssg.ts";
import { createJssgBatchDispatch } from "../../execution/jssg-batch.ts";
import type { OperationExecutor } from "../../execution/operation-executor.ts";
import { executeShell } from "../../execution/shell.ts";
import {
  PROTOCOL_VERSION,
  type OperationCompletion,
  type OperationRequest,
} from "../../core/protocol.ts";
import { NODE_RUNTIME, UnsupportedInRuntimeError } from "../../runtime/capabilities.ts";
import { getHost, type Host } from "../../core/host.ts";
import { NODE_HOST } from "../node-host.ts";
import { defaultAgentPath } from "./agent-bin.ts";
import { createNapiJssgEngine, createNapiSelectHost } from "./node-jssg.ts";

export interface NodeExecutorOptions {
  /** Repository root: `shell` cwd and JSSG target root. */
  cwd: string;
  artifacts: ArtifactStore;
  /** Added to the environment of `shell` commands. */
  env?: Record<string, string>;
  /**
   * TypeSafe SDK settings for `assessment`; unset fields fall back to the
   * SDK's `TYPESAFE_*` variables and defaults.
   */
  assessment?: AssessmentExecutorOptions;
  /** The `codemod-agent` runner. Default `defaultAgentPath()`. */
  agentBin?: string;
  /** Environment the agent allowlist reads from. Default: the host's. */
  hostEnv?: Readonly<Record<string, string | undefined>>;
  /** Wall-clock limit for one external agent; see `AgentExecutionOptions`. */
  externalAgentTimeoutMs?: number;
}

/** Executes `jssg`, `shell`, and `assessment` in this process, and `agent` in a runner process. */
export class NodeExecutor implements OperationExecutor {
  readonly #dispatch: JssgDispatch;
  /**
   * The installed host; the Node host gains the addon's native file
   * selection unless `CODEMOD_NATIVE_SELECT=0`. Another installed host (a
   * test double) is used as is.
   */
  readonly #host: Host;

  constructor(private readonly options: NodeExecutorOptions) {
    const host = getHost();
    this.#host =
      process.env.CODEMOD_NATIVE_SELECT === "0" || host !== NODE_HOST
        ? host
        : createNapiSelectHost(host);
    this.#dispatch = createJssgBatchDispatch({ engine: createNapiJssgEngine() });
  }

  async execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion> {
    const { cwd, artifacts, env } = this.options;
    const operation = request.operation;
    switch (operation.kind) {
      case "shell":
        return await executeShell({ cwd, ...(env === undefined ? {} : { env }) }, request, signal);
      case "jssg":
        return await executeJssgCommand({
          cwd,
          artifacts,
          commandId: request.commandId,
          operation,
          signal,
          host: this.#host,
          dispatch: this.#dispatch,
        });
      case "agent": {
        const { agentBin, hostEnv, externalAgentTimeoutMs } = this.options;
        return await executeAgent(
          {
            bin: agentBin ?? defaultAgentPath(),
            cwd,
            host: this.#host,
            ...(env === undefined ? {} : { env }),
            ...(hostEnv === undefined ? {} : { hostEnv }),
            ...(externalAgentTimeoutMs === undefined ? {} : { externalAgentTimeoutMs }),
          },
          request,
          signal,
        );
      }
      case "assessment":
        return await executeAssessmentOperation(
          this.options.assessment ?? {},
          cwd,
          request,
          signal,
        );
      default: {
        const unsupported = `operation kind '${(operation as { kind: string }).kind}'`;
        return {
          protocolVersion: PROTOCOL_VERSION,
          commandId: request.commandId,
          status: "failed",
          error: {
            message: new UnsupportedInRuntimeError(
              NODE_RUNTIME.name,
              unsupported,
              request.commandId,
            ).message,
            details: {
              phase: "config",
              runtime: NODE_RUNTIME.name,
              unsupported,
              repositoryMayBeModified: false,
            },
          },
        };
      }
    }
  }
}

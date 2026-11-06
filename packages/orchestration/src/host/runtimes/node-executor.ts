/**
 * The executor of the `node` runtime (`node.ts`): `jssg` in-process over
 * the napi-rs ast-grep addon (`@codemod.com/jssg-napi`), through the
 * portable orchestrator (`execution/jssg.ts::executeJssgCommand` with the
 * in-process dispatch of `execution/jssg-inprocess.ts`); `shell` through the
 * host (`execution/shell.ts::executeShell`); everything else refused.
 */
import type { ArtifactStore } from "../../bundle/build.ts";
import { executeJssgCommand, type JssgDispatch } from "../../execution/jssg.ts";
import { createInProcessJssgDispatch } from "../../execution/jssg-inprocess.ts";
import type { OperationExecutor } from "../../execution/operation-executor.ts";
import { executeShell } from "../../execution/shell.ts";
import {
  PROTOCOL_VERSION,
  type OperationCompletion,
  type OperationRequest,
} from "../../core/protocol.ts";
import { NODE_RUNTIME, UnsupportedInRuntimeError } from "../../runtime/capabilities.ts";
import { createNapiJssgEngine } from "./node-jssg.ts";

export interface NodeExecutorOptions {
  /** Repository root: `shell` cwd and JSSG target root. */
  cwd: string;
  artifacts: ArtifactStore;
  /** Added to the environment of `shell` commands. */
  env?: Record<string, string>;
  /**
   * Optional timers (milliseconds): `jssg.select` (walk + reads),
   * `jssg.stage`, `jssg.commit` from `executeJssgCommand`; `jssg.parse`
   * (off-thread), `jssg.parse.wait` (blocked on it), `jssg.load`,
   * `jssg.transform`, `jssg.convert` from the batch runner; `jssg.dispatch`
   * (the whole batch); `jssg.command` (all of it); and `shell`.
   */
  profile?: (phase: string, ms: number) => void;
}

/** Executes `jssg` and `shell` in this process; refuses everything else. */
export class NodeExecutor implements OperationExecutor {
  readonly #dispatch: JssgDispatch;

  constructor(private readonly options: NodeExecutorOptions) {
    const { profile } = options;
    const dispatch = createInProcessJssgDispatch({
      engine: createNapiJssgEngine(),
      ...(profile === undefined
        ? {}
        : { profile: (phase: string, ms: number) => profile(`jssg.${phase}`, ms) }),
    });
    this.#dispatch =
      profile === undefined
        ? dispatch
        : async (request, signal) => {
            const start = performance.now();
            try {
              return await dispatch(request, signal);
            } finally {
              profile("jssg.dispatch", performance.now() - start);
            }
          };
  }

  async execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion> {
    const { cwd, artifacts, env, profile } = this.options;
    const start = performance.now();
    const operation = request.operation;
    try {
      switch (operation.kind) {
        case "shell":
          return await executeShell(
            { cwd, ...(env === undefined ? {} : { env }) },
            request,
            signal,
          );
        case "jssg":
          return await executeJssgCommand({
            cwd,
            artifacts,
            commandId: request.commandId,
            operation,
            signal,
            dispatch: this.#dispatch,
            ...(profile === undefined
              ? {}
              : { profile: (phase: string, ms: number) => profile(`jssg.${phase}`, ms) }),
          });
        default: {
          const unsupported = `operation kind '${operation.kind}'`;
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
    } finally {
      profile?.(
        operation.kind === "jssg" ? "jssg.command" : operation.kind,
        performance.now() - start,
      );
    }
  }
}

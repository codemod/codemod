/**
 * A test executor that echoes instead of executing: `shell`
 * answers `{ stdout: JSON.stringify({ request }) }`, and `jssg` goes through
 * the real orchestrator (`executeJssgCommand`: artifact lookup, selection,
 * reading, commit) with a dispatch that appends `// echo\n` to every file
 * and outputs `{ path, context, input }` (the request context without its
 * files). Every answer waits `delayMs` first, honouring the abort signal, so
 * a test can cancel a command in flight.
 */
import type { ArtifactStore } from "../src/core/artifacts.ts";
import { executeJssgCommand, type JssgDispatch } from "../src/execution/jssg.ts";
import {
  PROTOCOL_VERSION,
  type FileOutcome,
  type Json,
  type OperationCompletion,
  type OperationExecutor,
  type OperationRequest,
} from "../src/index.ts";

const pause = (ms: number, signal?: AbortSignal) =>
  new Promise<void>((resolve) => {
    if (ms <= 0 || signal?.aborted) return resolve();
    const timer = setTimeout(resolve, ms);
    signal?.addEventListener("abort", () => (clearTimeout(timer), resolve()), { once: true });
  });

const echoDispatch: JssgDispatch = async (request) => {
  const { files = [], ...context } = request.context ?? {};
  const outcomes: FileOutcome[] = files.map((file) => ({
    path: file.path,
    edits: [{ path: file.path, content: `${file.content}// echo\n` }],
    output: {
      path: file.path,
      context: context as Json,
      input: (request.operation as { input?: Json }).input ?? null,
    },
  }));
  return {
    protocolVersion: PROTOCOL_VERSION,
    commandId: request.commandId,
    status: "succeeded",
    output: { files: outcomes as unknown as Json },
  };
};

export function echoExecutor(options: {
  cwd: string;
  artifacts?: ArtifactStore;
  delayMs?: number;
}): OperationExecutor {
  return {
    async execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion> {
      await pause(options.delayMs ?? 0, signal);
      if (signal?.aborted) {
        return {
          protocolVersion: PROTOCOL_VERSION,
          commandId: request.commandId,
          status: "cancelled",
          error: { message: "aborted" },
        };
      }
      if (request.operation.kind === "jssg") {
        return executeJssgCommand({
          cwd: options.cwd,
          ...(options.artifacts === undefined ? {} : { artifacts: options.artifacts }),
          commandId: request.commandId,
          operation: request.operation,
          signal,
          dispatch: echoDispatch,
        });
      }
      return {
        protocolVersion: PROTOCOL_VERSION,
        commandId: request.commandId,
        status: "succeeded",
        output: { stdout: JSON.stringify({ request }) },
      };
    },
  };
}

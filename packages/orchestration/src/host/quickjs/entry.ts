/**
 * The entry of a `quickjs` workflow bundle (built by
 * `crates/orchestration/src/build` for `codemod workflow run -w`), evaluated once by the Rust
 * host (`crates/orchestration/src/host`) in its single QuickJS context.
 *
 * Evaluating this module installs, before any workflow code runs: the
 * then-propagating async context (`async-context.ts`), the QuickJS `Host`
 * over the Rust primitives (`setHost`, and `globalThis[HOST_GLOBAL]`), and a
 * `console` writing to the host's stderr. The generated bundle entry then
 * calls `runBundled(workflowModule, artifacts, request)` from its exported
 * `main`, which runs the workflow with the portable control plane (replay,
 * history, scheduling) and resolves with
 * the JSON text the host prints.
 *
 * Runs inside QuickJS: Node-free (`tests/portable.test.ts`).
 */
import {
  executableRequiresInput,
  isExecutable,
  type Executable,
} from "../../authoring/composition.ts";
import type { JssgArtifact } from "../../core/artifacts.ts";
import { HOST_GLOBAL, setHost } from "../../core/host.ts";
import { MemoryHistoryStore } from "../../core/history.ts";
import type { Json } from "../../core/json.ts";
import {
  PROTOCOL_VERSION,
  type OperationCompletion,
  type OperationRequest,
} from "../../core/protocol.ts";
import {
  executeAssessmentOperation,
  type AssessmentExecutorOptions,
} from "../../execution/assessment.ts";
import { AGENT_BIN_ENV, executeAgent } from "../../execution/agent.ts";
import { executeJssgCommand, type JssgDispatch } from "../../execution/jssg.ts";
import type { OperationExecutor } from "../../execution/operation-executor.ts";
import { executeShell } from "../../execution/shell.ts";
import { run } from "../../runtime/run.ts";
import { installPromiseContextPropagation } from "./async-context.ts";
import { createQuickjsDispatch } from "./dispatch.ts";
import { createQuickjsFetch } from "./fetch.ts";
import { createQuickjsConsole, createQuickjsHost } from "./host.ts";
import { ABORT_GLOBAL, quickjsNative, type QuickjsNative } from "./native.ts";

/** One artifact as the bundle embeds it, keyed by hash. */
export interface BundledArtifact {
  name: string;
  source: string;
}

/** What the Rust host passes to `main`. */
export interface QuickjsRequest {
  /** Canonical repository root. */
  target: string;
  /** Root input as JSON text; absent when no input was given. */
  input?: string;
  /** A `History` (JSON text) to resume from. */
  history?: string;
}

const globals = globalThis as Record<string, unknown>;
const native: QuickjsNative = quickjsNative();
installPromiseContextPropagation();
const host = createQuickjsHost(native);
setHost(host);
globals[HOST_GLOBAL] = host;
globals.console = createQuickjsConsole(native);

/** `jssg` in context, `shell` through the Rust spawn; everything else refused. */
class QuickjsExecutor implements OperationExecutor {
  constructor(
    private readonly cwd: string,
    private readonly artifacts: Map<string, JssgArtifact>,
    private readonly dispatch: JssgDispatch,
    private readonly assessment: AssessmentExecutorOptions,
  ) {}

  async execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion> {
    const operation = request.operation;
    switch (operation.kind) {
      case "shell":
        return await executeShell({ cwd: this.cwd, host }, request, signal);
      case "jssg":
        return await executeJssgCommand({
          cwd: this.cwd,
          artifacts: this.artifacts,
          commandId: request.commandId,
          operation,
          signal,
          host,
          dispatch: this.dispatch,
        });
      case "agent":
        return await executeAgent(
          { bin: host.env()[AGENT_BIN_ENV], cwd: this.cwd, host },
          request,
          signal,
        );
      case "assessment":
        return await executeAssessmentOperation(this.assessment, this.cwd, request, signal);
      default:
        return {
          protocolVersion: PROTOCOL_VERSION,
          commandId: request.commandId,
          status: "failed",
          error: {
            message: `unknown operation kind '${(operation as { kind: string }).kind}'`,
            details: { phase: "config", repositoryMayBeModified: false },
          },
        };
    }
  }
}

/** Run the bundled workflow; resolves with `{ ok, output | error, history }` as JSON text. */
export async function runBundled(
  workflow: Record<string, unknown>,
  bundled: Record<string, BundledArtifact>,
  request: QuickjsRequest,
): Promise<string> {
  const controller = new AbortController();
  globals[ABORT_GLOBAL] = () => controller.abort();
  const artifacts = new Map<string, JssgArtifact>(
    Object.entries(bundled).map(([hash, artifact]) => [
      hash,
      { hash, name: artifact.name, source: artifact.source, origin: artifact.name },
    ]),
  );
  const executor = new QuickjsExecutor(request.target, artifacts, createQuickjsDispatch(native), {
    fetch: createQuickjsFetch(native),
  });
  let store: MemoryHistoryStore | undefined;
  const result: {
    ok: boolean;
    output?: unknown;
    error?: string;
    history?: unknown;
  } = { ok: false };
  try {
    store =
      request.history === undefined
        ? new MemoryHistoryStore()
        : MemoryHistoryStore.fromJSON(request.history);
    const executable = workflow.default;
    if (!isExecutable(executable)) {
      throw new Error(
        "workflow module must default-export a shell or jssg step, a step invocation, dynamic(...), sequence(...), or parallel(...)",
      );
    }
    const hasInput = request.input !== undefined;
    if (!hasInput && executableRequiresInput(executable)) {
      throw new Error("workflow requires an input value; pass --input <json>");
    }
    const outcome = await run(
      executable as Executable,
      {
        executor,
        history: store,
        signal: controller.signal,
        ...(hasInput ? { input: JSON.parse(request.input!) as Json } : {}),
      } as Parameters<typeof run>[1],
    );
    result.ok = true;
    result.output = outcome.output === undefined ? null : outcome.output;
  } catch (error) {
    result.error = error instanceof Error ? error.message : String(error);
  }
  if (store !== undefined) result.history = store.toJSON();
  return JSON.stringify(result);
}

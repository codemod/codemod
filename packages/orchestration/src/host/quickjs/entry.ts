/**
 * The entry of a `quickjs` workflow bundle (`codemod-workflow build
 * --runtime=quickjs`, `host/quickjs-bundle.ts`), evaluated once by the Rust
 * host (`crates/orchestration-quickjs-host`) in its single QuickJS context.
 *
 * Evaluating this module installs, before any workflow code runs: the
 * then-propagating async context (`async-context.ts`), the QuickJS `Host`
 * over the Rust primitives (`setHost`, and `globalThis[HOST_GLOBAL]`), and a
 * `console` writing to the host's stderr. The generated bundle entry then
 * calls `runBundled(workflowModule, artifacts, request)` from its exported
 * `main`, which runs the workflow with the portable control plane (replay,
 * history, scheduling, the `quickjs` capability contract) and resolves with
 * the JSON text the host prints.
 *
 * Runs inside QuickJS: Node-free (`tests/portable.test.ts`).
 */
import {
  executableRequiresInput,
  isExecutable,
  type Executable,
} from "../../authoring/composition.ts";
import type { JssgArtifact } from "../../bundle/build.ts";
import { HOST_GLOBAL, setHost } from "../../core/host.ts";
import { MemoryHistoryStore } from "../../core/history.ts";
import type { Json } from "../../core/json.ts";
import {
  PROTOCOL_VERSION,
  type OperationCompletion,
  type OperationRequest,
} from "../../core/protocol.ts";
import { executeJssgCommand } from "../../execution/jssg.ts";
import type { OperationExecutor } from "../../execution/operation-executor.ts";
import { executeShell } from "../../execution/shell.ts";
import { QUICKJS_RUNTIME, UnsupportedInRuntimeError } from "../../runtime/capabilities.ts";
import { run } from "../../runtime/run.ts";
import { installPromiseContextPropagation } from "./async-context.ts";
import { createQuickjsDispatch, type QuickjsDispatch } from "./dispatch.ts";
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
  timings?: boolean;
  /** Host milestones, milliseconds since the host process started. */
  startup?: { runtimeReadyMs: number; bundleLoadedMs: number };
}

/** Per-kind wall time inside the executor, for the timer breakdown. */
interface ExecutorTimings {
  /** Target-root resolution, file selection, and reads (before dispatch). */
  selectMs: number;
  /** Of `selectMs`: target root and the walk (`profile` phase `select`, whole ms). */
  walkMs: number;
  /** Of `selectMs`: reading the selected sources (`profile` phase `read`, whole ms). */
  readMs: number;
  /** Validation, conflict checks, and writes (after dispatch). */
  commitMs: number;
  /** The batch runner (parse, transform, convert). */
  dispatchMs: number;
  /** Shell operations end to end. */
  shellMs: number;
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
  readonly timings: ExecutorTimings = {
    selectMs: 0,
    walkMs: 0,
    readMs: 0,
    commitMs: 0,
    dispatchMs: 0,
    shellMs: 0,
  };

  constructor(
    private readonly cwd: string,
    private readonly artifacts: Map<string, JssgArtifact>,
    private readonly jssg: QuickjsDispatch,
  ) {}

  async execute(request: OperationRequest, signal?: AbortSignal): Promise<OperationCompletion> {
    const operation = request.operation;
    const started = native.hrtime();
    switch (operation.kind) {
      case "shell": {
        try {
          return await executeShell({ cwd: this.cwd, host }, request, signal);
        } finally {
          this.timings.shellMs += native.hrtime() - started;
        }
      }
      case "jssg": {
        let dispatchStart = started;
        let dispatchEnd = started;
        try {
          return await executeJssgCommand({
            cwd: this.cwd,
            artifacts: this.artifacts,
            commandId: request.commandId,
            operation,
            signal,
            host,
            profile: (phase, ms) => {
              if (phase === "select") this.timings.walkMs += ms;
              else if (phase === "read") this.timings.readMs += ms;
            },
            dispatch: async (batch, batchSignal) => {
              dispatchStart = native.hrtime();
              try {
                return await this.jssg.dispatch(batch, batchSignal);
              } finally {
                dispatchEnd = native.hrtime();
              }
            },
          });
        } finally {
          const end = native.hrtime();
          if (dispatchEnd > dispatchStart) {
            this.timings.selectMs += dispatchStart - started;
            this.timings.dispatchMs += dispatchEnd - dispatchStart;
            this.timings.commitMs += end - dispatchEnd;
          } else {
            this.timings.selectMs += end - started;
          }
        }
      }
      default:
        // `run()` refuses these before they get here (`CapabilityExecutor`).
        return {
          protocolVersion: PROTOCOL_VERSION,
          commandId: request.commandId,
          status: "failed",
          error: {
            message: new UnsupportedInRuntimeError("quickjs", `operation kind '${operation.kind}'`)
              .message,
            details: { phase: "config", runtime: "quickjs", repositoryMayBeModified: false },
          },
        };
    }
  }
}

const round = (ms: number) => Math.round(ms * 10) / 10;

/** Run the bundled workflow; resolves with `{ ok, output | error, history, timings? }` as JSON text. */
export async function runBundled(
  workflow: Record<string, unknown>,
  bundled: Record<string, BundledArtifact>,
  request: QuickjsRequest,
): Promise<string> {
  const mainStarted = native.hrtime();
  const controller = new AbortController();
  globals[ABORT_GLOBAL] = () => controller.abort();
  const artifacts = new Map<string, JssgArtifact>(
    Object.entries(bundled).map(([hash, artifact]) => [
      hash,
      { hash, name: artifact.name, source: artifact.source, origin: artifact.name },
    ]),
  );
  const jssg = createQuickjsDispatch(native);
  const executor = new QuickjsExecutor(request.target, artifacts, jssg);
  let store: MemoryHistoryStore | undefined;
  const result: {
    ok: boolean;
    output?: unknown;
    error?: string;
    history?: unknown;
    timings?: Json;
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
        runtime: QUICKJS_RUNTIME,
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
  if (request.timings === true) {
    const stats = native.jssgStats();
    const end = native.hrtime();
    result.timings = {
      hostStartupMs: round(request.startup?.runtimeReadyMs ?? 0),
      bundleLoadMs: round(
        (request.startup?.bundleLoadedMs ?? 0) - (request.startup?.runtimeReadyMs ?? 0),
      ),
      runMs: round(end - mainStarted),
      selectAndReadMs: round(executor.timings.selectMs),
      walkMs: round(executor.timings.walkMs),
      readMs: round(executor.timings.readMs),
      dispatchMs: round(executor.timings.dispatchMs),
      parseMs: round(stats.prepareMs),
      moduleLoadMs: round(jssg.timings.loadMs),
      transformMs: round(jssg.timings.transformMs),
      convertMs: round(stats.finishMs),
      commitMs: round(executor.timings.commitMs),
      shellMs: round(executor.timings.shellMs),
      files: stats.files,
      skipped: stats.skipped,
      totalMs: round(end),
    };
  }
  return JSON.stringify(result);
}

/**
 * Test harness: scripted completions, run/replay, and inspection helpers.
 * Exercises only the public runtime API.
 */
import { CollectingSink } from "../core/events.ts";
import type { OperationExecutor } from "../execution/executor.ts";
import {
  MemoryHistoryStore,
  completions,
  scheduledCommands,
  type History,
  type ScheduledCommand,
} from "../core/history.ts";
import type { Json } from "../core/json.ts";
import {
  PROTOCOL_VERSION,
  type CompletionError,
  type CompletionStatus,
  type OperationCompletion,
  type OperationRequest,
} from "../core/protocol.ts";
import type { AdmissionScheduler } from "../execution/scheduler.ts";
import type { Executable, ExecutableOutput, StageInput } from "../authoring/composition.ts";
import { run, type RunOptions, type RunResult } from "../runtime/run.ts";

export class Outcome {
  constructor(
    readonly status: Exclude<CompletionStatus, "succeeded">,
    readonly error: CompletionError,
  ) {}
}

export const failed = (message = "scripted failure", exitCode?: number): Outcome =>
  new Outcome("failed", exitCode === undefined ? { message } : { message, exitCode });
export const cancelled = (message = "scripted cancellation"): Outcome =>
  new Outcome("cancelled", { message });
export const unknown = (message = "scripted unknown outcome"): Outcome =>
  new Outcome("unknown", { message });

/**
 * A plain value succeeds. For shell runnables, non-string values are
 * JSON-encoded into stdout. A function may return a promise, which is how
 * tests hold an operation open while others queue behind it.
 */
export type ScriptValue =
  | Json
  | Outcome
  | ((request: OperationRequest) => Json | Outcome | Promise<Json | Outcome>);

export interface HarnessOptions {
  /** Keyed by command id. */
  results?: Record<string, ScriptValue>;
  /** Used when no `results` entry matches. */
  fallback?: (request: OperationRequest) => Json | Outcome | Promise<Json | Outcome>;
  /** Start from a serialized or in-memory history instead of an empty one. */
  history?: string | History;
  /** Bounded admission for the run; defaults to the host-derived scheduler. */
  scheduler?: AdmissionScheduler;
  signal?: AbortSignal;
}

export interface HarnessRun<R> extends RunResult<R> {
  commands: ScheduledCommand[];
  completions: Map<string, OperationCompletion>;
}

/**
 * Per-run options: the root input, required exactly when the root declares
 * one (see `RootInput`), so `h.run(noInput)` and `h.run(needsInput, { input })`
 * both typecheck and `h.run(needsInput)` does not.
 */
export type HarnessRunArgs<I> = undefined extends I
  ? [options?: { input?: I }]
  : [options: { input: I }];

export interface Harness {
  /** Requests that actually reached the executor (never populated by replay). */
  readonly executed: OperationRequest[];
  readonly store: MemoryHistoryStore;
  readonly events: CollectingSink;
  run<T extends Executable>(
    executable: T,
    ...args: HarnessRunArgs<StageInput<T>>
  ): Promise<HarnessRun<ExecutableOutput<T>>>;
  serialize(): string;
  /** A fresh harness that starts from this harness's serialized history. */
  reload(overrides?: Omit<HarnessOptions, "history">): Harness;
}

export function createHarness(options: HarnessOptions = {}): Harness {
  const store =
    typeof options.history === "string"
      ? MemoryHistoryStore.fromJSON(options.history)
      : new MemoryHistoryStore(options.history);
  const executed: OperationRequest[] = [];
  const events = new CollectingSink();
  const executor: OperationExecutor = {
    async execute(request) {
      executed.push(request);
      return toCompletion(request, await script(options, request));
    },
  };

  return {
    executed,
    store,
    events,
    async run<T extends Executable>(executable: T, ...args: HarnessRunArgs<StageInput<T>>) {
      const [runOptions] = args as [{ input?: unknown }?];
      const result = await run<T>(executable, {
        executor,
        history: store,
        events,
        ...(options.scheduler === undefined ? {} : { scheduler: options.scheduler }),
        ...(options.signal === undefined ? {} : { signal: options.signal }),
        ...(runOptions === undefined || !("input" in runOptions)
          ? {}
          : { input: runOptions.input }),
      } as RunOptions<StageInput<T>>);
      return {
        ...result,
        commands: scheduledCommands(result.history),
        completions: completions(result.history),
      } as HarnessRun<ExecutableOutput<T>>;
    },
    serialize: () => store.serialize(),
    reload: (overrides = {}) =>
      createHarness({ ...options, ...overrides, history: store.serialize() }),
  };
}

function script(
  options: HarnessOptions,
  request: OperationRequest,
): Json | Outcome | Promise<Json | Outcome> {
  const entry = options.results?.[request.commandId];
  if (entry !== undefined) return typeof entry === "function" ? entry(request) : entry;
  if (options.fallback) return options.fallback(request);
  throw new Error(`harness: no scripted result for command '${request.commandId}'`);
}

function toCompletion(request: OperationRequest, value: Json | Outcome): OperationCompletion {
  const base = { protocolVersion: PROTOCOL_VERSION, commandId: request.commandId } as const;
  if (value instanceof Outcome) return { ...base, status: value.status, error: value.error };
  if (request.operation.kind === "shell") {
    const stdout = typeof value === "string" ? value : JSON.stringify(value);
    return { ...base, status: "succeeded", output: { stdout } };
  }
  return { ...base, status: "succeeded", output: value };
}

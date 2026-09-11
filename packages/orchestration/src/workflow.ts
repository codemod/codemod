/**
 * Procedural workflows. The body is ordinary async TypeScript that awaits
 * commands (`await inspect()`), plans, and parallel groups. It receives no
 * context argument: the runtime for the current run is bound to the body's
 * async continuations (see `context.ts`). Determinism is NOT enforced by a
 * sandbox in this prototype: it is validated after the fact by replaying
 * history and comparing issued commands (see README).
 */
import { operationOf, type Command } from "./command.ts";
import { withRuntime, type Runtime } from "./context.ts";
import { OperationError } from "./errors.ts";
import { CollectingSink, type EventSink } from "./events.ts";
import type { OperationExecutor } from "./executor.ts";
import { ReplayGate, type CommandGate } from "./gate.ts";
import {
  MemoryHistoryStore,
  type History,
  type HistoryStore,
  type ScheduledCommand,
} from "./history.ts";
import type { Json } from "./json.ts";
import { isPlan, runPlan, type Plan } from "./plan.ts";
import { validate } from "./schema.ts";

export interface Workflow<R> {
  readonly type: "workflow";
  readonly body: () => PromiseLike<R>;
}

export function workflow<R>(body: () => PromiseLike<R>): Workflow<R> {
  return { type: "workflow", body };
}

/**
 * Something `run()` can execute. Named `Executable` rather than "target" so it
 * is not confused with a JSSG invocation `Target`, which is file selection
 * inside one command, not something that runs (see DESIGN.md, "Targeting").
 */
export type Executable = Workflow<unknown> | Plan;
export type ExecutableOutput<T> =
  T extends Workflow<infer R> ? R : T extends Plan<infer O> ? O : never;

export interface RunOptions {
  executor: OperationExecutor;
  /** Defaults to an empty in-memory store. */
  history?: HistoryStore;
  events?: EventSink;
  /**
   * Aborts the operation in flight. Its completion becomes `cancelled` (no
   * repository change) or `unknown` (a JSSG commit had started) and is
   * recorded; the awaited command rejects with `OperationError`.
   */
  signal?: AbortSignal;
}

export interface RunResult<R> {
  output: R;
  /** True when the final output was already recorded and this run only replayed. */
  replayed: boolean;
  history: History;
}

export async function run<T extends Executable>(
  executable: T,
  options: RunOptions,
): Promise<RunResult<ExecutableOutput<T>>> {
  const store = options.history ?? new MemoryHistoryStore();
  const events = options.events ?? new CollectingSink();
  const gate = new ReplayGate(await store.load(), store, options.executor, events, options.signal);
  const runtime = new WorkflowRuntime(gate);
  const subject: Executable = executable;
  let output: unknown;
  let bodyError: unknown;
  let bodySucceeded = false;
  try {
    // The async wrapper keeps the runtime bound while the body's result,
    // which may itself be a command, is adopted.
    output = await withRuntime(runtime, async () =>
      isPlan(subject) ? await runPlan(runtime, subject) : await subject.body(),
    );
    bodySucceeded = true;
  } catch (error) {
    bodyError = error;
  }

  const pending = runtime.close();
  await Promise.allSettled(pending.inFlight);
  if (!bodySucceeded) throw bodyError;
  const unawaited = pending.inFlight.length + pending.unissued.length;
  if (unawaited > 0) {
    throw new Error(
      `workflow body returned without awaiting ${unawaited} operation(s): ${pending.ids.join(", ")}`,
    );
  }

  const { replayed } = await gate.finish((output === undefined ? null : output) as Json);
  return {
    output: output as ExecutableOutput<T>,
    replayed,
    history: await store.load(),
  };
}

/**
 * One run's runtime. Tracks every command created while the body is active
 * and every command issued, so a body that returns before awaiting its work
 * is refused finalization instead of leaving results to land afterwards.
 */
class WorkflowRuntime implements Runtime {
  readonly #gate: CommandGate;
  readonly #created = new Set<Command>();
  readonly #issued = new Map<Command, Promise<unknown>>();
  readonly #inFlight = new Set<Promise<unknown>>();
  #closed = false;

  constructor(gate: CommandGate) {
    this.#gate = gate;
  }

  created(command: Command): void {
    if (!this.#closed) this.#created.add(command);
  }

  issue<O>(command: Command<O>): Promise<O> {
    const existing = this.#issued.get(command);
    if (existing !== undefined) return existing as Promise<O>;
    if (this.#closed) {
      return Promise.reject(
        new Error(`command '${command.id}' was issued after the workflow body returned`),
      );
    }
    const operation = this.#resolve(command);
    this.#issued.set(command, operation);
    this.#inFlight.add(operation);
    void operation.then(
      () => this.#inFlight.delete(operation),
      () => this.#inFlight.delete(operation),
    );
    return operation;
  }

  close(): { inFlight: Promise<unknown>[]; unissued: Command[]; ids: string[] } {
    this.#closed = true;
    const unissued = [...this.#created].filter((command) => !this.#issued.has(command));
    const inFlightIds = [...this.#issued]
      .filter(([, operation]) => this.#inFlight.has(operation))
      .map(([command]) => command.id);
    return {
      inFlight: [...this.#inFlight],
      unissued,
      ids: [...inFlightIds, ...unissued.map((command) => command.id)],
    };
  }

  async #resolve<O>(command: Command<O>): Promise<O> {
    const { runnable, id } = command;
    const input = await validate(runnable.input, command.input, `input of '${id}'`);
    const scheduled: ScheduledCommand = {
      id,
      runnable: runnable.name,
      kind: runnable.kind,
      operation: operationOf(command, input),
    };
    if (input !== undefined) scheduled.input = input as Json;
    const completion = await this.#gate.resolve(scheduled);
    if (completion.status !== "succeeded") {
      throw new OperationError(id, completion.status, completion.error);
    }
    return runnable.decode(completion.output);
  }
}

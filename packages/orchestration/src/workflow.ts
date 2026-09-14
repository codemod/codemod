/**
 * Runtime and lifecycle for arbitrary workflows and static composition. The
 * runtime for the current run is bound to async continuations (see
 * `context.ts`). Determinism is NOT enforced by a
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
import {
  runStage,
  type Executable,
  type ExecutableOutput,
  type StageInput,
} from "./composition.ts";
import { AdmissionScheduler, SchedulingExecutor } from "./scheduler.ts";
import { validate } from "./schema.ts";

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
  /**
   * Bounded admission for this run. Defaults to a fresh `AdmissionScheduler`
   * with host-derived capacity: `parallel()` declares eligibility, this decides
   * how much of it actually overlaps. Pass one to observe it or to fix the
   * capacity in a test; it is host configuration, never workflow authoring.
   */
  scheduler?: AdmissionScheduler;
}

export interface RunResult<R> {
  output: R;
  /** True when the final output was already recorded and this run only replayed. */
  replayed: boolean;
  history: History;
}

export async function run<T extends Executable>(
  executable: T & (undefined extends StageInput<T> ? unknown : never),
  options: RunOptions,
): Promise<RunResult<ExecutableOutput<T>>> {
  const store = options.history ?? new MemoryHistoryStore();
  const events = options.events ?? new CollectingSink();
  // Only commands the gate actually executes pass through here, so a replayed
  // command never takes a permit.
  const executor = new SchedulingExecutor(
    options.executor,
    options.scheduler ?? new AdmissionScheduler(),
    events,
  );
  const gate = new ReplayGate(await store.load(), store, executor, events, options.signal);
  const runtime = new WorkflowRuntime(gate);
  let output: unknown;
  let bodyError: unknown;
  let bodySucceeded = false;
  try {
    // The async wrapper keeps the runtime bound while nested workflows and
    // composition nodes run and while their thenable results are adopted.
    output = await withRuntime(runtime, async () => runStage(runtime, executable, undefined));
    bodySucceeded = true;
  } catch (error) {
    bodyError = error;
  }

  const pending = runtime.close();
  await Promise.allSettled(pending.inFlight);
  if (!bodySucceeded) throw bodyError;
  const unawaited = pending.ids.length;
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
  readonly #compositions = new Map<object, readonly string[]>();
  readonly #startedCompositions = new Set<object>();
  readonly #claimed = new Set<Command>();
  readonly #issued = new Map<Command, Promise<unknown>>();
  readonly #inFlight = new Set<Promise<unknown>>();
  #closed = false;

  constructor(gate: CommandGate) {
    this.#gate = gate;
  }

  created(command: Command): void {
    if (!this.#closed) this.#created.add(command);
  }

  createdComposition(composition: object, commandIds: readonly string[]): void {
    if (!this.#closed) this.#compositions.set(composition, commandIds);
  }

  startedComposition(composition: object): void {
    if (!this.#closed) this.#startedCompositions.add(composition);
  }

  claimed(command: Command): void {
    if (!this.#closed) this.#claimed.add(command);
  }

  issue<O>(command: Command<O>, concurrent = false): Promise<O> {
    const existing = this.#issued.get(command);
    if (existing !== undefined) return existing as Promise<O>;
    if (this.#closed) {
      return Promise.reject(
        new Error(`command '${command.id}' was issued after the workflow body returned`),
      );
    }
    const operation = this.#resolve(command, concurrent);
    this.#issued.set(command, operation);
    this.#inFlight.add(operation);
    void operation.then(
      () => this.#inFlight.delete(operation),
      () => this.#inFlight.delete(operation),
    );
    return operation;
  }

  close(): { inFlight: Promise<unknown>[]; ids: string[] } {
    this.#closed = true;
    const unissued = [...this.#created].filter(
      (command) => !this.#issued.has(command) && !this.#claimed.has(command),
    );
    const inFlightIds = [...this.#issued]
      .filter(([, operation]) => this.#inFlight.has(operation))
      .map(([command]) => command.id);
    const unstartedIds = [...this.#compositions]
      .filter(([composition]) => !this.#startedCompositions.has(composition))
      .flatMap(([composition, ids]) =>
        ids.length > 0 ? ids : [`${String((composition as { type?: unknown }).type)} (opaque)`],
      );
    return {
      inFlight: [...this.#inFlight],
      ids: [
        ...new Set([...inFlightIds, ...unissued.map((command) => command.id), ...unstartedIds]),
      ],
    };
  }

  async #resolve<O>(command: Command<O>, concurrent: boolean): Promise<O> {
    const { runnable, id } = command;
    const input = await validate(runnable.input, command.input, `input of '${id}'`);
    const scheduled: ScheduledCommand = {
      id,
      runnable: runnable.name,
      kind: runnable.kind,
      ...(concurrent ? { concurrent: true as const } : {}),
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

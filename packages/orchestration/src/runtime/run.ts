/**
 * Runtime and lifecycle for arbitrary workflows and static composition. The
 * runtime for the current run is bound to async continuations (see
 * `authoring/context.ts`). Determinism is NOT enforced by a
 * sandbox in this prototype: it is validated after the fact by replaying
 * history and comparing issued commands (see README).
 */
import { operationOf, type Command } from "../authoring/command.ts";
import { withRuntime, type Runtime } from "../authoring/context.ts";
import { OperationError } from "../core/errors.ts";
import { CollectingSink, type EventSink } from "../core/events.ts";
import type { OperationExecutor } from "../execution/executor.ts";
import { ReplayGate, type CommandGate } from "./gate.ts";
import {
  MemoryHistoryStore,
  type History,
  type HistoryStore,
  type ScheduledCommand,
} from "../core/history.ts";
import type { Json } from "../core/json.ts";
import {
  runStage,
  type Executable,
  type ExecutableOutput,
  type StageInput,
} from "../authoring/composition.ts";
import { AdmissionScheduler, SchedulingExecutor } from "../execution/scheduler.ts";
import { validate } from "../authoring/schema.ts";

export interface RunSettings {
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
   * with host-derived capacity that reports to `events`: `parallel()` declares
   * eligibility, this decides how much of it actually overlaps. Pass one to
   * observe it, to pause and resume admission from the host, or to fix the
   * capacity in a test; it is host configuration, never workflow authoring.
   */
  scheduler?: AdmissionScheduler;
}

/**
 * The root's flowing input. Required when the root declares one (a runnable
 * with an input schema, a dynamic step with a parameter, a static node whose
 * first stage or any member does); otherwise optional and ignored by bound
 * commands. `null` is a value; an absent `input` is `undefined`.
 */
export type RootInput<I> = undefined extends I ? { input?: I } : { input: I };

export type RunOptions<I = unknown> = RunSettings & RootInput<I>;

export interface RunResult<R> {
  output: R;
  /** True when the final output was already recorded and this run only replayed. */
  replayed: boolean;
  history: History;
}

export async function run<T extends Executable>(
  executable: T,
  options: RunOptions<StageInput<T>>,
): Promise<RunResult<ExecutableOutput<T>>> {
  const store = options.history ?? new MemoryHistoryStore();
  const events = options.events ?? new CollectingSink();
  // Only commands the gate actually executes pass through here, so a replayed
  // command never takes a permit.
  const executor = new SchedulingExecutor(
    options.executor,
    options.scheduler ?? new AdmissionScheduler({ events }),
    events,
  );
  const gate = new ReplayGate(await store.load(), store, executor, events, options.signal);
  const runtime = new RunRuntime(gate);
  let output: unknown;
  let bodyError: unknown;
  let bodySucceeded = false;
  try {
    // The async wrapper keeps the runtime bound while nested workflows and
    // composition nodes run and while their thenable results are adopted.
    const input: unknown = (options as { input?: unknown }).input;
    output = await withRuntime(runtime, async () => runStage(runtime, executable, input));
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
class RunRuntime implements Runtime {
  readonly #gate: CommandGate;
  readonly #created = new Set<Command<unknown, unknown>>();
  readonly #compositions = new Map<object, readonly string[]>();
  readonly #startedCompositions = new Set<object>();
  readonly #claimed = new Set<Command<unknown, unknown>>();
  readonly #issued = new Map<Command<unknown, unknown>, Promise<unknown>>();
  readonly #inFlight = new Set<Promise<unknown>>();
  #closed = false;

  constructor(gate: CommandGate) {
    this.#gate = gate;
  }

  created(command: Command<unknown, unknown>): void {
    if (!this.#closed) this.#created.add(command);
  }

  createdComposition(composition: object, commandIds: readonly string[]): void {
    if (!this.#closed) this.#compositions.set(composition, commandIds);
  }

  startedComposition(composition: object): void {
    if (!this.#closed) this.#startedCompositions.add(composition);
  }

  claimed(command: Command<unknown, unknown>): void {
    if (!this.#closed) this.#claimed.add(command);
  }

  issue<O>(
    command: Command<O, unknown>,
    concurrent = false,
    flow?: { input: unknown },
    dynamicId?: string,
  ): Promise<O> {
    const existing = this.#issued.get(command);
    if (existing !== undefined) return existing as Promise<O>;
    if (this.#closed) {
      return Promise.reject(
        new Error(`command '${command.id}' was issued after the workflow body returned`),
      );
    }
    const operation = this.#resolve(command, concurrent, flow, dynamicId);
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

  async #resolve<O>(
    command: Command<O, unknown>,
    concurrent: boolean,
    flow?: { input: unknown },
    dynamicId?: string,
  ): Promise<O> {
    const { runnable, id } = command;
    if (command.inputMode === "flow" && flow === undefined) {
      throw new Error(
        `command '${id}' requires flowing input; place it in sequence()/parallel() or invoke it with { input }`,
      );
    }
    const rawInput = command.inputMode === "flow" ? flow?.input : command.input;
    const input = await validate(runnable.input, rawInput, `input of '${id}'`);
    const scheduled: ScheduledCommand = {
      id,
      runnable: runnable.name,
      kind: runnable.kind,
      ...(concurrent ? { concurrent: true as const } : {}),
      operation: operationOf(command, input),
    };
    if (input !== undefined) scheduled.input = input as Json;
    const completion = await this.#gate.resolve(scheduled, dynamicId);
    if (completion.status !== "succeeded") {
      throw new OperationError(id, completion.status, completion.error);
    }
    return runnable.decode(completion.output, scheduled.operation);
  }
}

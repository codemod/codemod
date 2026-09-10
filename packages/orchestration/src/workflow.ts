/**
 * Procedural workflows. The body is ordinary async TypeScript; the only
 * capability it receives is `w.run`. Determinism is NOT enforced by a sandbox
 * in this prototype: it is validated after the fact by replaying history and
 * comparing issued commands (see README).
 */
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
import { isParallel, isPlan, type Plan, type PlanStep } from "./plan.ts";
import type { Runnable } from "./runnable.ts";
import { validate } from "./schema.ts";

export type RunArgs<I> = I extends void
  ? [options?: { id?: string }]
  : [options: { id?: string; input: I }];

export interface WorkflowContext {
  run<I, O>(runnable: Runnable<I, O>, ...args: RunArgs<I>): Promise<O>;
  run<Outputs extends unknown[]>(plan: Plan<Outputs>): Promise<Outputs>;
}

export interface Workflow<R> {
  readonly type: "workflow";
  readonly body: (w: WorkflowContext) => Promise<R>;
}

export function workflow<R>(body: (w: WorkflowContext) => Promise<R>): Workflow<R> {
  return { type: "workflow", body };
}

/**
 * Something `run()` can execute. Named `Executable` rather than "target" so it
 * is not confused with the proposed `target()` file-selection modifier, which
 * is not itself an operation (see DESIGN.md, "Targeting").
 */
export type Executable = Workflow<unknown> | Plan;
export type ExecutableOutput<T> =
  T extends Workflow<infer R> ? R : T extends Plan<infer O> ? O : never;

export interface RunOptions {
  executor: OperationExecutor;
  /** Defaults to an empty in-memory store. */
  history?: HistoryStore;
  events?: EventSink;
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
  const gate = new ReplayGate(await store.load(), store, options.executor, events);
  const context = new Context(gate);
  const runnable: Executable = executable;
  let output: unknown;
  let bodyError: unknown;
  let bodySucceeded = false;
  try {
    output = isPlan(runnable) ? await context.run(runnable) : await runnable.body(context);
    bodySucceeded = true;
  } catch (error) {
    bodyError = error;
  }

  const pending = context.close();
  await Promise.allSettled(pending);
  if (!bodySucceeded) throw bodyError;
  if (pending.length > 0) {
    throw new Error(`workflow body returned without awaiting ${pending.length} operation(s)`);
  }

  const { replayed } = await gate.finish((output === undefined ? null : output) as Json);
  return {
    output: output as ExecutableOutput<T>,
    replayed,
    history: await store.load(),
  };
}

class Context implements WorkflowContext {
  readonly #gate: CommandGate;
  readonly #inFlight = new Set<Promise<unknown>>();
  #closed = false;

  constructor(gate: CommandGate) {
    this.#gate = gate;
  }

  run<I, O>(runnable: Runnable<I, O>, ...args: RunArgs<I>): Promise<O>;
  run<Outputs extends unknown[]>(plan: Plan<Outputs>): Promise<Outputs>;
  run(
    subject: Runnable<unknown, unknown> | Plan,
    options?: { id?: string; input?: unknown },
  ): Promise<unknown> {
    if (this.#closed) {
      return Promise.reject(new Error("w.run called after the workflow body returned"));
    }
    const operation = isPlan(subject) ? this.#runPlan(subject) : this.#runOne(subject, options);
    this.#inFlight.add(operation);
    void operation.then(
      () => this.#inFlight.delete(operation),
      () => this.#inFlight.delete(operation),
    );
    return operation;
  }

  close(): Promise<unknown>[] {
    this.#closed = true;
    return [...this.#inFlight];
  }

  async #runPlan(plan: Plan): Promise<unknown[]> {
    const outputs: unknown[] = [];
    for (const step of plan.steps) outputs.push(await this.#runStep(step));
    return outputs;
  }

  #runStep(step: PlanStep): Promise<unknown> {
    if (isParallel(step)) return Promise.all(step.members.map((member) => this.#runOne(member)));
    return this.#runOne(step);
  }

  async #runOne(runnable: Runnable<unknown, unknown>, options?: { id?: string; input?: unknown }) {
    const id = options?.id ?? runnable.name;
    const input = await validate(runnable.input, options?.input, `input of '${id}'`);
    const command: ScheduledCommand = {
      id,
      runnable: runnable.name,
      kind: runnable.kind,
      operation: runnable.toOperation(input),
    };
    if (input !== undefined) command.input = input as Json;
    const completion = await this.#gate.resolve(command);
    if (completion.status !== "succeeded") {
      throw new OperationError(id, completion.status, completion.error);
    }
    return runnable.decode(completion.output);
  }
}

/**
 * Commands: one invocation of a runnable, created by calling the runnable
 * (`inspect()`, `migrate({ input, target, id })`). Creating a command never
 * executes anything. It is plain data that `sequence()` and `parallel()` can hold,
 * and it is a thenable: awaiting it inside an active workflow issues it to the
 * runtime, which replays it from history or executes it and records it.
 */
import { InvocationError, TargetValidationError } from "../core/errors.ts";
import { activeRuntime, NoActiveRunError, type Runtime } from "./context.ts";
import type { Operation, Target } from "../core/protocol.ts";
import type { JssgRunnable, Runnable } from "./runnable.ts";
import { normalizeTarget } from "./target.ts";

declare const flowingInput: unique symbol;
export type CommandInputMode = "none" | "flow" | "bound";

export interface Command<O = unknown, I = void> extends PromiseLike<O> {
  readonly type: "command";
  /** Command id used for history and replay: `options.id`, else the runnable name. */
  readonly id: string;
  readonly runnable: Runnable<unknown, O>;
  /** Whether this invocation has no input, consumes static flow, or binds a value. */
  readonly inputMode: CommandInputMode;
  /** Raw bound input; validated when issued. Undefined for `none` and `flow`. */
  readonly input: unknown;
  /** Present only for JSSG invocations that were given a target. */
  readonly target?: Target;
  /** Type-only carrier for the input static composition must provide. */
  readonly [flowingInput]?: I;
}

export interface FlowInvocation {
  id?: string;
}

export interface BoundInvocation<I> extends FlowInvocation {
  input: I;
}

/** Invocation options for shell, agent, and assessment steps: identity and optional bound data. */
export type Invocation<I> = FlowInvocation | BoundInvocation<I>;

/** A JSSG invocation may also select the repository area it applies to. */
export type JssgInvocation<I> = Invocation<I> & { target?: Target };

const COMMON_FIELDS: readonly string[] = ["id", "input"];

/** Validate invocation options and classify how the command receives input. */
export function createCommand<O>(
  runnable: Runnable<unknown, O>,
  options: unknown,
): Command<O, unknown> {
  const where = `${runnable.kind} '${runnable.name}'`;
  if (options === undefined) {
    return new CommandImpl(
      runnable,
      runnable.name,
      runnable.input === undefined ? "none" : "flow",
      undefined,
      undefined,
    );
  }
  if (typeof options !== "object" || options === null || Array.isArray(options)) {
    throw new InvocationError(where, "invocation options must be an object");
  }
  const record = options as Record<string, unknown>;
  for (const key of Object.keys(record)) {
    if (COMMON_FIELDS.includes(key)) continue;
    if (key === "target") {
      if (runnable.kind === "jssg") continue;
      throw new TargetValidationError(
        where,
        `${runnable.kind} does not accept a target; only JSSG invocations select files`,
      );
    }
    throw new InvocationError(where, `unknown invocation field '${key}'`);
  }
  if (record.id !== undefined && (typeof record.id !== "string" || record.id === "")) {
    throw new InvocationError(where, "id must be a non-empty string");
  }
  const target = record.target === undefined ? undefined : normalizeTarget(record.target, where);
  const hasInput = Object.hasOwn(record, "input");
  return new CommandImpl(
    runnable,
    (record.id as string | undefined) ?? runnable.name,
    hasInput ? "bound" : runnable.input === undefined ? "none" : "flow",
    hasInput ? record.input : undefined,
    target,
  );
}

export function isCommand(value: unknown): value is Command<unknown, unknown> {
  return value instanceof CommandImpl;
}

/** Build the wire operation for a command from its validated input. */
export function operationOf(command: Command<unknown, unknown>, input: unknown): Operation {
  if (command.target === undefined) return command.runnable.toOperation(input);
  if (command.runnable.kind !== "jssg") {
    throw new TargetValidationError(
      `${command.runnable.kind} '${command.runnable.name}'`,
      "only JSSG invocations carry a target",
    );
  }
  return (command.runnable as JssgRunnable<unknown, unknown>).toOperation(input, command.target);
}

class CommandImpl<O> implements Command<O, unknown> {
  readonly type = "command" as const;

  constructor(
    readonly runnable: Runnable<unknown, O>,
    readonly id: string,
    readonly inputMode: CommandInputMode,
    readonly input: unknown,
    readonly target: Target | undefined,
  ) {
    if (target === undefined) delete (this as { target?: Target }).target;
    activeRuntime()?.created(this);
  }

  // Being awaitable is the point of a command: awaiting issues it to the active workflow.
  // oxlint-disable-next-line unicorn/no-thenable
  then<R1 = O, R2 = never>(
    onfulfilled?: ((value: O) => R1 | PromiseLike<R1>) | null,
    onrejected?: ((reason: unknown) => R2 | PromiseLike<R2>) | null,
  ): Promise<R1 | R2> {
    return issue(this, activeRuntime()).then(onfulfilled, onrejected);
  }
}

/** Issue a command to a runtime, or reject when no workflow is active. */
export function issue<O>(command: Command<O, unknown>, runtime: Runtime | undefined): Promise<O> {
  if (runtime === undefined) {
    return Promise.reject(new NoActiveRunError(`command '${command.id}'`));
  }
  return runtime.issue(command);
}

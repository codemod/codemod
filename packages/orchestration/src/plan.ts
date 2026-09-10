/**
 * Fixed plans: an ordered list of commands, optionally containing explicit
 * parallel groups. A plan is validated at construction and compiled
 * to a small JSON IR so a future Rust scheduler could consume it directly.
 *
 * Plans and parallel groups are data, like commands. Awaiting one inside an
 * active workflow runs it through that workflow's runtime; `run(plan)` runs a
 * plan on its own. `parallel()` accepts members spread (a fixed group) or as
 * one array (a group built dynamically inside a workflow).
 */
import { isCommand, type Command } from "./command.ts";
import { activeRuntime, NoActiveWorkflowError, type Runtime } from "./context.ts";
import { PlanValidationError } from "./errors.ts";
import type { Target } from "./protocol.ts";
import { isRunnable, type Runnable } from "./runnable.ts";

/** A command, or a runnable that needs no input (its default command is used). */
export type PlanMember = Command | Runnable<void, unknown>;

export interface Parallel<Outputs extends unknown[] = unknown[]> extends PromiseLike<Outputs> {
  readonly type: "parallel";
  readonly members: readonly Command[];
}

export type PlanStep = PlanMember | Parallel;

export interface Plan<Outputs extends unknown[] = unknown[]> extends PromiseLike<Outputs> {
  readonly type: "plan";
  readonly steps: readonly (Command | Parallel)[];
  readonly ir: PlanIr;
}

/** JSON IR of a plan. */
export interface PlanIr {
  version: 1;
  steps: PlanIrStep[];
}

/** `target` is present only for targeted JSSG members. */
export interface PlanIrEntry {
  id: string;
  name: string;
  kind: string;
  target?: Target;
}

export type PlanIrStep =
  | ({ type: "run" } & PlanIrEntry)
  | { type: "parallel"; members: PlanIrEntry[] };

type MemberOutput<M> =
  M extends Command<infer O> ? O : M extends Runnable<void, infer O> ? O : never;
type StepOutput<S> = S extends Parallel<infer O> ? O : MemberOutput<S>;
type StepOutputs<S extends readonly PlanStep[]> = { -readonly [K in keyof S]: StepOutput<S[K]> };
type MemberOutputs<M extends readonly PlanMember[]> = {
  -readonly [K in keyof M]: MemberOutput<M[K]>;
};

/**
 * Explicit assertion that members have no ordering dependency. Members are
 * started in declaration order and their outputs are returned in that order.
 */
export function parallel<const M extends readonly PlanMember[]>(
  ...members: M
): Parallel<MemberOutputs<M>>;
export function parallel<const M extends readonly PlanMember[]>(
  members: M,
): Parallel<MemberOutputs<M>>;
export function parallel(...args: readonly (PlanMember | readonly PlanMember[])[]): Parallel {
  const members = args.length === 1 && Array.isArray(args[0]) ? args[0] : (args as PlanMember[]);
  if (members.length === 0) throw new PlanValidationError("parallel group has no members");
  return new ParallelImpl(members.map(toCommand));
}

export function plan<const S extends readonly PlanStep[]>(...steps: S): Plan<StepOutputs<S>> {
  if (steps.length === 0) throw new PlanValidationError("plan has no steps");
  const seen = new Set<string>();
  const entry = (command: Command): PlanIrEntry => {
    if (seen.has(command.id)) {
      throw new PlanValidationError(
        `command id '${command.id}' appears twice; pass an explicit { id } to one invocation`,
      );
    }
    seen.add(command.id);
    return {
      id: command.id,
      name: command.runnable.name,
      kind: command.runnable.kind,
      ...(command.target === undefined ? {} : { target: command.target }),
    };
  };
  const ir: PlanIr = { version: 1, steps: [] };
  const normalized: (Command | Parallel)[] = [];
  for (const step of steps) {
    if (isParallel(step)) {
      ir.steps.push({ type: "parallel", members: step.members.map(entry) });
      normalized.push(step);
    } else {
      const command = toCommand(step);
      ir.steps.push({ type: "run", ...entry(command) });
      normalized.push(command);
    }
  }
  return new PlanImpl<StepOutputs<S>>(normalized, ir);
}

/** A bare runnable in a plan stands for its default command: no input, id = name. */
function toCommand(member: PlanMember): Command {
  if (isCommand(member)) return member;
  if (isRunnable(member)) return (member as unknown as () => Command)();
  throw new PlanValidationError("plan members must be commands or runnables");
}

export function isParallel(value: unknown): value is Parallel {
  return value instanceof ParallelImpl;
}

export function isPlan(value: unknown): value is Plan {
  return value instanceof PlanImpl;
}

/** Run a plan's steps in order on a runtime; parallel members start together. */
export function runPlan(runtime: Runtime, plan: Plan): Promise<unknown[]> {
  return sequence(plan.steps.map((step) => () => runStep(runtime, step)));
}

export function runParallel(runtime: Runtime, group: Parallel): Promise<unknown[]> {
  return Promise.all(group.members.map((member) => runtime.issue(member)));
}

function runStep(runtime: Runtime, step: Command | Parallel): Promise<unknown> {
  return isParallel(step) ? runParallel(runtime, step) : runtime.issue(step);
}

async function sequence(steps: (() => Promise<unknown>)[]): Promise<unknown[]> {
  const outputs: unknown[] = [];
  for (const step of steps) outputs.push(await step());
  return outputs;
}

function inActiveWorkflow<T>(what: string, body: (runtime: Runtime) => Promise<T>): Promise<T> {
  const runtime = activeRuntime();
  if (runtime === undefined) return Promise.reject(new NoActiveWorkflowError(what));
  return body(runtime);
}

class ParallelImpl<Outputs extends unknown[]> implements Parallel<Outputs> {
  readonly type = "parallel" as const;
  constructor(readonly members: readonly Command[]) {}

  // Awaiting a group inside a workflow runs it there; see `Command.then`.
  // oxlint-disable-next-line unicorn/no-thenable
  then<R1 = Outputs, R2 = never>(
    onfulfilled?: ((value: Outputs) => R1 | PromiseLike<R1>) | null,
    onrejected?: ((reason: unknown) => R2 | PromiseLike<R2>) | null,
  ): Promise<R1 | R2> {
    return inActiveWorkflow("parallel group", (runtime) =>
      runParallel(runtime, this).then((outputs) => outputs as Outputs),
    ).then(onfulfilled, onrejected);
  }
}

class PlanImpl<Outputs extends unknown[]> implements Plan<Outputs> {
  readonly type = "plan" as const;
  constructor(
    readonly steps: readonly (Command | Parallel)[],
    readonly ir: PlanIr,
  ) {}

  // Awaiting a plan inside a workflow runs it there; see `Command.then`.
  // oxlint-disable-next-line unicorn/no-thenable
  then<R1 = Outputs, R2 = never>(
    onfulfilled?: ((value: Outputs) => R1 | PromiseLike<R1>) | null,
    onrejected?: ((reason: unknown) => R2 | PromiseLike<R2>) | null,
  ): Promise<R1 | R2> {
    return inActiveWorkflow("plan", (runtime) =>
      runPlan(runtime, this).then((outputs) => outputs as Outputs),
    ).then(onfulfilled, onrejected);
  }
}

/** Static sequential and parallel composition around wire-operation runnables. */
import { createCommand, isCommand, type Command } from "./command.ts";
import { activeRuntime, NoActiveRunError, withRuntime, type Runtime } from "./context.ts";
import { CompositionValidationError, DuplicateCommandIdError } from "../core/errors.ts";
import type { Target } from "../core/protocol.ts";
import { isRunnable, type Runnable } from "./runnable.ts";
import { dynamicRequiresInput, isDynamic, runDynamic, type Dynamic } from "./dynamic.ts";

type AnyRunnable = Runnable<unknown, unknown>;
type AnyDynamic = Dynamic<never, unknown>;
type AnyCommand = Command<unknown, unknown>;
type AnyParallel = ParallelNode<never, unknown[]>;
type AnySequence = SequenceNode<never, unknown>;
declare const stageInput: unique symbol;
declare const stageOutput: unique symbol;
const inputRequired = Symbol("inputRequired");

/** Invocations consume flow unless they have no input or explicitly bind one. */
export type Stage = AnyCommand | AnyDynamic | AnyParallel | AnySequence;

export type StageInput<S> =
  S extends Command<unknown, infer I>
    ? I
    : S extends Runnable<infer I, unknown>
      ? I
      : S extends Dynamic<infer I, unknown>
        ? I
        : S extends ParallelNode<infer I, unknown[]>
          ? I
          : S extends SequenceNode<infer I, unknown>
            ? I
            : never;

export type StageOutput<S> =
  S extends Command<infer O, infer _I>
    ? O
    : S extends Runnable<infer _I, infer O>
      ? O
      : S extends Dynamic<infer _I, infer O>
        ? O
        : S extends ParallelNode<infer _I, infer O>
          ? O
          : S extends SequenceNode<infer _I, infer O>
            ? O
            : never;

type StageOutputs<S extends readonly Stage[]> = { -readonly [K in keyof S]: StageOutput<S[K]> };
type First<S extends readonly Stage[]> = S extends readonly [infer F, ...unknown[]] ? F : never;
type Last<S extends readonly Stage[]> = S extends readonly [...unknown[], infer L] ? L : never;
type FlowingInput<S> = [StageInput<S>] extends [void] ? never : StageInput<S>;
type UnionToIntersection<U> = (U extends unknown ? (input: U) => void : never) extends (
  input: infer I,
) => void
  ? I
  : never;
type ParallelInput<S extends readonly Stage[]> = [FlowingInput<S[number]>] extends [never]
  ? unknown
  : UnionToIntersection<FlowingInput<S[number]>>;

type IsCompatibleSequence<S extends readonly Stage[]> = S extends readonly [
  infer A extends Stage,
  infer B extends Stage,
  ...infer Rest extends Stage[],
]
  ? [StageInput<B>] extends [void]
    ? IsCompatibleSequence<readonly [B, ...Rest]>
    : [StageOutput<A>] extends [StageInput<B>]
      ? IsCompatibleSequence<readonly [B, ...Rest]>
      : false
  : true;
type CheckedSequence<S extends readonly Stage[]> = IsCompatibleSequence<S> extends true ? S : never;

export interface OperationIr {
  type: "operation";
  id: string;
  name: string;
  kind: string;
  input: "none" | "flow" | "bound";
  target?: Target;
}

export interface DynamicIr {
  /** Opaque until dynamic functions are bundled for the sandbox. */
  type: "dynamic";
}

export interface SequenceIr {
  type: "sequence";
  stages: CompositionIrNode[];
}

export interface ParallelIr {
  type: "parallel";
  members: CompositionIrNode[];
}

export type CompositionIrNode = OperationIr | DynamicIr | SequenceIr | ParallelIr;
export interface CompositionIr<Root extends SequenceIr | ParallelIr = SequenceIr | ParallelIr> {
  version: 1;
  root: Root;
}

interface SequenceNode<I, O> {
  readonly type: "sequence";
  readonly stages: readonly Stage[];
  readonly ir: CompositionIr<SequenceIr>;
  readonly [stageInput]?: (input: I) => void;
  readonly [stageOutput]?: O;
  readonly [inputRequired]: boolean;
}

interface ParallelNode<I, Outputs extends unknown[]> {
  readonly type: "parallel";
  readonly members: readonly Stage[];
  readonly ir: CompositionIr<ParallelIr>;
  readonly [stageInput]?: (input: I) => void;
  readonly [stageOutput]?: Outputs;
  readonly [inputRequired]: boolean;
}

type AwaitableNode<I, O> = undefined extends I ? PromiseLike<O> : object;
export type Sequence<I = unknown, O = unknown> = SequenceNode<I, O> & AwaitableNode<I, O>;
export type Parallel<I = unknown, Outputs extends unknown[] = unknown[]> = ParallelNode<
  I,
  Outputs
> &
  AwaitableNode<I, Outputs>;

/**
 * Anything `run()` accepts as a root. A bare runnable is an implicit single invocation.
 */
export type Executable = AnyRunnable | AnyCommand | AnyDynamic | AnyParallel | AnySequence;
export type ExecutableOutput<T> = StageOutput<T>;

/** Run stages in order, passing each output to the next stage. */
export function sequence<const S extends readonly [Stage, ...Stage[]]>(
  ...stages: S & CheckedSequence<S>
): Sequence<StageInput<First<S>>, StageOutput<Last<S>>> {
  if (stages.length === 0) throw new CompositionValidationError("sequence has no stages");
  validateStages(stages);
  validateUniqueIds(stages);
  return new SequenceImpl(stages, stageRequiresInput(stages[0])) as Sequence<
    StageInput<First<S>>,
    StageOutput<Last<S>>
  >;
}

/** Run every member with the same input and return outputs in declaration order. */
export function parallel<const S extends readonly Stage[]>(
  ...members: S
): Parallel<ParallelInput<S>, StageOutputs<S>>;
export function parallel<const S extends readonly Stage[]>(
  members: S,
): Parallel<ParallelInput<S>, StageOutputs<S>>;
export function parallel(...args: readonly (Stage | readonly Stage[])[]): Parallel {
  const members = args.length === 1 && Array.isArray(args[0]) ? [...args[0]] : (args as Stage[]);
  if (members.length === 0) throw new CompositionValidationError("parallel group has no members");
  validateStages(members);
  validateUniqueIds(members);
  return new ParallelImpl(members, members.some(stageRequiresInput)) as Parallel;
}

export function isSequence(value: unknown): value is AnySequence {
  return value instanceof SequenceImpl;
}

export function isParallel(value: unknown): value is AnyParallel {
  return value instanceof ParallelImpl;
}

export function isExecutable(value: unknown): value is Executable {
  return (
    isRunnable(value) ||
    isCommand(value) ||
    isDynamic(value) ||
    isSequence(value) ||
    isParallel(value)
  );
}

/**
 * Whether a root needs an input value before it can run: a flow invocation,
 * a dynamic step whose body takes a parameter, or a static node
 * whose first stage (sequence) or any member (parallel) does.
 */
export function executableRequiresInput(executable: Executable): boolean {
  if (isRunnable(executable)) return executable.input !== undefined;
  return stageRequiresInput(executable);
}

export async function runStage(
  runtime: Runtime,
  stage: Executable,
  input: unknown,
): Promise<unknown> {
  if (isRunnable(stage)) {
    const command = createCommand(stage, stage.input === undefined ? undefined : { input });
    return runtime.issue(command);
  }
  if (isCommand(stage)) {
    return runtime.issue(stage, false, stage.inputMode === "flow" ? { input } : undefined);
  }
  if (isDynamic(stage)) return runDynamic(stage, input as never);
  if (isSequence(stage)) return runSequence(runtime, stage, input);
  if (isParallel(stage)) return runParallel(runtime, stage, input);
  throw new CompositionValidationError("stage is not runnable");
}

export async function runSequence(
  runtime: Runtime,
  definition: AnySequence,
  input: unknown,
): Promise<unknown> {
  startComposition(runtime, definition);
  let output = input;
  for (const stage of definition.stages) output = await runStage(runtime, stage, output);
  return output;
}

export function runParallel(
  runtime: Runtime,
  definition: AnyParallel,
  input: unknown,
): Promise<unknown[]> {
  startComposition(runtime, definition);
  const child = concurrentRuntime(runtime);
  return settleParallel(
    definition.members.map((member) => withRuntime(child, () => runStage(child, member, input))),
  );
}

async function settleParallel(members: Promise<unknown>[]): Promise<unknown[]> {
  let firstError: unknown;
  let failed = false;
  const settled = await Promise.allSettled(
    members.map((member) =>
      member.catch((error: unknown) => {
        if (!failed) {
          failed = true;
          firstError = error;
        }
        throw error;
      }),
    ),
  );
  if (failed) throw firstError;
  return settled.map((result) => (result as PromiseFulfilledResult<unknown>).value);
}

function concurrentRuntime(runtime: Runtime): Runtime {
  return {
    created: (command) => runtime.created(command),
    createdComposition: (composition, commandIds) =>
      runtime.createdComposition(composition, commandIds),
    startedComposition: (composition) => runtime.startedComposition(composition),
    claimed: (command) => runtime.claimed(command),
    issue: (command, _concurrent, flow) => runtime.issue(command, true, flow),
  };
}

function validateStages(stages: readonly Stage[]): void {
  for (const stage of stages) {
    if (isCommand(stage) || isDynamic(stage)) continue;
    if (isSequence(stage) || isParallel(stage)) continue;
    throw new CompositionValidationError(
      "members must be command invocations, workflows, sequences, or parallel groups",
    );
  }
}

function stageRequiresInput(stage: Stage): boolean {
  if (isCommand(stage)) return stage.inputMode === "flow";
  if (isDynamic(stage)) return dynamicRequiresInput(stage);
  return stage[inputRequired];
}

function validateUniqueIds(stages: readonly Stage[]): void {
  const seen = new Set<string>();
  const visit = (node: CompositionIrNode): void => {
    if (node.type === "operation") {
      if (seen.has(node.id)) throw new DuplicateCommandIdError(node.id);
      seen.add(node.id);
      return;
    }
    if (node.type === "dynamic") return;
    for (const child of node.type === "sequence" ? node.stages : node.members) visit(child);
  };
  for (const stage of stages) visit(irOf(stage));
}

function operationIds(node: CompositionIrNode): string[] {
  if (node.type === "operation") return [node.id];
  if (node.type === "dynamic") return [];
  return (node.type === "sequence" ? node.stages : node.members).flatMap(operationIds);
}

function startComposition(runtime: Runtime, definition: AnySequence | AnyParallel): void {
  runtime.startedComposition(definition);
  const stages = isSequence(definition) ? definition.stages : definition.members;
  for (const stage of stages) {
    if (isCommand(stage)) runtime.claimed(stage);
    else if (isSequence(stage) || isParallel(stage)) startComposition(runtime, stage);
  }
}

function irOf(stage: Stage): CompositionIrNode {
  if (isCommand(stage)) {
    return {
      type: "operation",
      id: stage.id,
      name: stage.runnable.name,
      kind: stage.runnable.kind,
      input: stage.inputMode,
      ...(stage.target === undefined ? {} : { target: stage.target }),
    };
  }
  if (isDynamic(stage)) return { type: "dynamic" };
  return stage.ir.root;
}

function inActiveDynamic<T>(what: string, body: (runtime: Runtime) => Promise<T>): Promise<T> {
  const runtime = activeRuntime();
  if (runtime === undefined) return Promise.reject(new NoActiveRunError(what));
  return body(runtime);
}

class SequenceImpl implements SequenceNode<never, unknown> {
  readonly type = "sequence" as const;
  readonly ir: CompositionIr<SequenceIr>;
  readonly [inputRequired]: boolean;
  // oxlint-disable-next-line unicorn/no-thenable
  readonly then?: PromiseLike<unknown>["then"];

  constructor(
    readonly stages: readonly Stage[],
    requiresInput: boolean,
  ) {
    this[inputRequired] = requiresInput;
    this.ir = { version: 1, root: { type: "sequence", stages: stages.map(irOf) } };
    activeRuntime()?.createdComposition(this, operationIds(this.ir.root));
    if (!requiresInput) {
      // oxlint-disable-next-line unicorn/no-thenable
      this.then = (onfulfilled, onrejected) =>
        inActiveDynamic("sequence", (runtime) => runSequence(runtime, this, undefined)).then(
          onfulfilled,
          onrejected,
        );
    }
  }
}

class ParallelImpl implements ParallelNode<never, unknown[]> {
  readonly type = "parallel" as const;
  readonly ir: CompositionIr<ParallelIr>;
  readonly [inputRequired]: boolean;
  // oxlint-disable-next-line unicorn/no-thenable
  readonly then?: PromiseLike<unknown[]>["then"];

  constructor(
    readonly members: readonly Stage[],
    requiresInput: boolean,
  ) {
    this[inputRequired] = requiresInput;
    this.ir = { version: 1, root: { type: "parallel", members: members.map(irOf) } };
    activeRuntime()?.createdComposition(this, operationIds(this.ir.root));
    if (!requiresInput) {
      // oxlint-disable-next-line unicorn/no-thenable
      this.then = (onfulfilled, onrejected) =>
        inActiveDynamic("parallel group", (runtime) => runParallel(runtime, this, undefined)).then(
          onfulfilled,
          onrejected,
        );
    }
  }
}

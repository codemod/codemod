/**
 * Fixed plans: an ordered list of runnables, optionally containing explicit
 * parallel groups. A plan is validated at construction and compiled
 * to a small JSON IR so a future Rust scheduler could consume it directly.
 */
import { PlanValidationError } from "./errors.ts";
import type { Target } from "./protocol.ts";
import type { JssgRunnable, Runnable } from "./runnable.ts";

export interface Parallel<Outputs extends unknown[] = unknown[]> {
  readonly type: "parallel";
  readonly members: readonly Runnable<void, unknown>[];
  readonly __outputs?: Outputs;
}

export type PlanStep = Runnable<void, unknown> | Parallel;

export interface Plan<Outputs extends unknown[] = unknown[]> {
  readonly type: "plan";
  readonly steps: readonly PlanStep[];
  readonly ir: PlanIr;
  readonly __outputs?: Outputs;
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

type StepOutput<S> =
  S extends Parallel<infer O> ? O : S extends Runnable<void, infer O> ? O : never;
type StepOutputs<S extends readonly PlanStep[]> = { -readonly [K in keyof S]: StepOutput<S[K]> };
type MemberOutputs<M extends readonly Runnable<void, unknown>[]> = {
  -readonly [K in keyof M]: M[K] extends Runnable<void, infer O> ? O : never;
};

/** Explicit assertion that members have no ordering dependency. */
export function parallel<const M extends readonly Runnable<void, unknown>[]>(
  ...members: M
): Parallel<MemberOutputs<M>> {
  if (members.length === 0) throw new PlanValidationError("parallel group has no members");
  return { type: "parallel", members };
}

export function plan<const S extends readonly PlanStep[]>(...steps: S): Plan<StepOutputs<S>> {
  if (steps.length === 0) throw new PlanValidationError("plan has no steps");
  const seen = new Set<string>();
  const use = (name: string) => {
    if (seen.has(name)) {
      throw new PlanValidationError(
        `runnable '${name}' appears twice; plans use runnable names as command ids`,
      );
    }
    seen.add(name);
    return name;
  };
  const entry = (runnable: Runnable<void, unknown>): PlanIrEntry => {
    const target = targetOf(runnable);
    return {
      id: use(runnable.name),
      name: runnable.name,
      kind: runnable.kind,
      ...(target === undefined ? {} : { target }),
    };
  };
  const ir: PlanIr = { version: 1, steps: [] };
  for (const step of steps) {
    if (isParallel(step)) {
      ir.steps.push({ type: "parallel", members: step.members.map(entry) });
    } else {
      ir.steps.push({ type: "run", ...entry(step) });
    }
  }
  return { type: "plan", steps, ir };
}

function targetOf(runnable: Runnable<void, unknown>): Target | undefined {
  return runnable.kind === "jssg" ? (runnable as JssgRunnable<void, unknown>).target : undefined;
}

export function isParallel(step: unknown): step is Parallel {
  return typeof step === "object" && step !== null && (step as Parallel).type === "parallel";
}

export function isPlan(value: unknown): value is Plan {
  return typeof value === "object" && value !== null && (value as Plan).type === "plan";
}

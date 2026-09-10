/**
 * Fixed plans: an ordered list of runnables, optionally containing explicit
 * read-only parallel groups. A plan is validated at construction and compiled
 * to a small JSON IR so a future Rust scheduler could consume it directly.
 */
import { PlanValidationError } from "./errors.ts";
import type { Runnable } from "./runnable.ts";

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

export type PlanIrStep =
  | { type: "run"; id: string; name: string; kind: string; readOnly: boolean }
  | { type: "parallel"; readOnly: true; members: { id: string; name: string; kind: string }[] };

type StepOutput<S> =
  S extends Parallel<infer O> ? O : S extends Runnable<void, infer O> ? O : never;
type StepOutputs<S extends readonly PlanStep[]> = { -readonly [K in keyof S]: StepOutput<S[K]> };
type MemberOutputs<M extends readonly Runnable<void, unknown>[]> = {
  -readonly [K in keyof M]: M[K] extends Runnable<void, infer O> ? O : never;
};

/** Explicit read-only parallel group. Every member must declare `readOnly: true`. */
export function parallel<const M extends readonly Runnable<void, unknown>[]>(
  ...members: M
): Parallel<MemberOutputs<M>> {
  if (members.length === 0) throw new PlanValidationError("parallel group has no members");
  for (const member of members) {
    if (!member.readOnly) {
      throw new PlanValidationError(
        `parallel member '${member.name}' is not read-only; parallel groups may only contain readOnly runnables`,
      );
    }
  }
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
  const ir: PlanIr = { version: 1, steps: [] };
  for (const step of steps) {
    if (isParallel(step)) {
      ir.steps.push({
        type: "parallel",
        readOnly: true,
        members: step.members.map((m) => ({ id: use(m.name), name: m.name, kind: m.kind })),
      });
    } else {
      ir.steps.push({
        type: "run",
        id: use(step.name),
        name: step.name,
        kind: step.kind,
        readOnly: step.readOnly,
      });
    }
  }
  return { type: "plan", steps, ir };
}

export function isParallel(step: unknown): step is Parallel {
  return typeof step === "object" && step !== null && (step as Parallel).type === "parallel";
}

export function isPlan(value: unknown): value is Plan {
  return typeof value === "object" && value !== null && (value as Plan).type === "plan";
}

// `codemod:workflow` in Node.js and Bun: the shared orchestrator, with the
// run looked up per call (see context.mjs).
//
// In the workflow body, `step`, `getState` and `setState` go to the run's
// orchestrator. Inside a function step or a `forEachFile` callback, the state
// functions see the state the step started with, and what they set becomes
// the step's state changes, as in the engine's isolates.

import { createHash } from "node:crypto";

import { current } from "./context.mts";
import { createTasks } from "./orchestrator.mts";
import type { Callback, Duration, Json, Runtime } from "./orchestrator.mts";

export { AwaitingApprovalError, CanceledError, StepError } from "./orchestrator.mts";

/** Identifies a callback given as a function, for replay checks. */
export function callbackId(fn: Callback): string {
  return `fn:${createHash("sha256").update(String(fn)).digest("hex").slice(0, 16)}`;
}

export const {
  __callback,
  forEachFile,
  shell,
  jssg,
  astGrep,
  shard,
  codemod,
  agent,
  installSkill,
  approval,
  branch,
  commit,
  pullRequest,
} = createTasks({ callbackId });

/** The run's orchestrator, for what only the workflow body may call. */
function body(what: string): Runtime {
  const scope = current(what);
  if (scope.step) {
    throw new Error(
      `${what} cannot be called inside a step or forEachFile callback; call it from the workflow body`,
    );
  }
  return scope.runtime;
}

export function step(...args: unknown[]): Promise<Json> {
  return body("step()").step(...args);
}

export function sleep(duration: Duration): Promise<Json> {
  return body("sleep()").sleep(duration);
}

export function waitForEvent(name: string, options?: { timeout?: Duration }): Promise<Json> {
  return body("waitForEvent()").waitForEvent(name, options);
}

// In a step or callback the state functions behave as in the engine's
// isolates: values are copied in and out, and what the step leaves different
// from the state it started with becomes its state changes.

export function getState(key?: string): unknown {
  const scope = current("getState()");
  if (!scope.step) return scope.runtime.getState(key);
  const state = scope.step.state;
  if (key === undefined) return structuredClone(Object.fromEntries(state));
  return state.has(key) ? structuredClone(state.get(key)) : undefined;
}

export function setState(key: string, value: unknown, persist = true): void {
  const scope = current("setState()");
  if (!scope.step) return scope.runtime.setState(key, value);
  const step = scope.step;
  step.state.set(key, stateValue(value));
  step.removed.delete(key);
  if (persist) step.unpersisted.delete(key);
  else step.unpersisted.add(key);
}

export function unsetState(key: string): void {
  const scope = current("unsetState()");
  if (!scope.step) return scope.runtime.unsetState(key);
  scope.step.state.delete(key);
  scope.step.removed.add(key);
}

/**
 * A value as state keeps it, the way the engine's isolates convert one:
 * what is not JSON becomes `null`, and objects keep their own enumerable
 * properties (so a `Date` or a `Map` becomes `{}`).
 */
function stateValue(value: unknown): Json {
  switch (typeof value) {
    case "boolean":
    case "string":
      return value;
    case "number":
      return Number.isFinite(value) ? value : null;
    case "bigint":
      return Number(value);
    case "object":
      if (value === null) return null;
      if (Array.isArray(value)) return value.map(stateValue);
      return Object.fromEntries(
        Object.entries(value as Record<string, unknown>).map(([key, item]) => [
          key,
          stateValue(item),
        ]),
      );
    default:
      return null;
  }
}

/** Callbacks run on one thread here, so a lock has nothing to wait for. */
export function acquireLock(name: string): () => void {
  const scope = current("acquireLock()");
  if (!scope.step) return scope.runtime.acquireLock(name);
  return () => {};
}

// Step outputs are kept for the transforms that still set them; nothing in a
// workflow reads them back but these functions.

export function setStepOutput(name: string, value: unknown): void {
  const scope = current("setStepOutput()");
  if (!scope.step) return scope.runtime.setStepOutput(name, value);
  scope.step.outputs.set(name, String(value));
}

export function getStepOutput(stepId: string, name: string): string | null {
  const scope = current("getStepOutput()");
  if (!scope.step) return scope.runtime.getStepOutput(stepId, name);
  return scope.step.outputs.get(name) ?? null;
}

export function getOrSetStepOutput(stepId: string, name: string, value: unknown): string {
  const scope = current("getOrSetStepOutput()");
  if (!scope.step) return scope.runtime.getOrSetStepOutput(stepId, name, value);
  const existing = scope.step.outputs.get(name);
  if (existing !== undefined) return existing;
  const text = String(value);
  scope.step.outputs.set(name, text);
  return text;
}

export function __evaluate(kind: string, expression: string, scope: unknown): unknown {
  return current("__evaluate()").runtime.__evaluate(kind, expression, scope);
}

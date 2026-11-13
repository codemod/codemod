// `codemod:workflow` in Node.js and Bun: the shared orchestrator, with the
// run looked up per call (see context.mjs).
//
// In the workflow body, `step`, `getState` and `setState` go to the run's
// orchestrator. Inside a function step or a `forEachFile` callback, the state
// functions see the state the step started with, and what they set becomes
// the step's state changes, as in the engine's isolates.

import { createHash } from "node:crypto";

import { current } from "./context.mjs";
import { createTasks } from "./orchestrator.mjs";

export {
  AwaitingApprovalError,
  CanceledError,
  NondeterminismError,
  StepError,
} from "./orchestrator.mjs";

/** Identifies a callback given as a function, for replay checks. */
export function callbackId(fn) {
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

export function step(labelOrTask, maybeTask) {
  const scope = current("step()");
  if (scope.step) {
    throw new Error(
      "step() cannot be called inside a step or forEachFile callback; start steps from the workflow body",
    );
  }
  return scope.runtime.step(labelOrTask, maybeTask);
}

// In a step or callback the state functions behave as in the engine's
// isolates: values are copied in and out, and what the step leaves different
// from the state it started with becomes its state changes.

export function getState(key) {
  const scope = current("getState()");
  if (!scope.step) return scope.runtime.getState(key);
  const state = scope.step.state;
  if (key === undefined) return structuredClone(Object.fromEntries(state));
  return state.has(key) ? structuredClone(state.get(key)) : undefined;
}

export function setState(key, value, persist = true) {
  const scope = current("setState()");
  if (!scope.step) return scope.runtime.setState(key, value);
  const step = scope.step;
  step.state.set(key, stateValue(value));
  step.removed.delete(key);
  if (persist) step.unpersisted.delete(key);
  else step.unpersisted.add(key);
}

export function unsetState(key) {
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
function stateValue(value) {
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
      return Object.fromEntries(Object.keys(value).map((key) => [key, stateValue(value[key])]));
    default:
      return null;
  }
}

/** Callbacks run on one thread here, so a lock has nothing to wait for. */
export function acquireLock(name) {
  const scope = current("acquireLock()");
  if (!scope.step) return scope.runtime.acquireLock(name);
  return () => {};
}

// Step outputs are kept for the transforms that still set them; nothing in a
// workflow reads them back but these functions.

export function setStepOutput(name, value) {
  const scope = current("setStepOutput()");
  if (!scope.step) return scope.runtime.setStepOutput(name, value);
  scope.step.outputs.set(name, String(value));
}

export function getStepOutput(stepId, name) {
  const scope = current("getStepOutput()");
  if (!scope.step) return scope.runtime.getStepOutput(stepId, name);
  return scope.step.outputs.get(name) ?? null;
}

export function getOrSetStepOutput(stepId, name, value) {
  const scope = current("getOrSetStepOutput()");
  if (!scope.step) return scope.runtime.getOrSetStepOutput(stepId, name, value);
  if (!scope.step.outputs.has(name)) scope.step.outputs.set(name, String(value));
  return scope.step.outputs.get(name);
}

export function __evaluate(kind, expression, scope) {
  return current("__evaluate()").runtime.__evaluate(kind, expression, scope);
}

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

export function getState(key) {
  const scope = current("getState()");
  if (!scope.step) return scope.runtime.getState(key);
  const state = scope.step.state;
  if (key === undefined) return Object.freeze(Object.fromEntries(state));
  return state.get(key);
}

export function setState(key, value, persist = true) {
  const scope = current("setState()");
  if (!scope.step) return scope.runtime.setState(key, value);
  if (value === undefined) return unsetState(key);
  const copy = JSON.parse(JSON.stringify(value));
  scope.step.state.set(key, copy);
  if (persist) scope.step.changes[key] = copy;
}

export function unsetState(key) {
  const scope = current("unsetState()");
  if (!scope.step) return scope.runtime.unsetState(key);
  scope.step.state.delete(key);
  scope.step.changes[key] = null;
}

/** Callbacks run on one thread here, so a lock has nothing to wait for. */
export function acquireLock(_name) {
  current("acquireLock()");
  return () => {};
}

export function __evaluate(kind, expression, scope) {
  return current("__evaluate()").runtime.__evaluate(kind, expression, scope);
}

function unsupported(name) {
  return () => {
    throw new Error(`${name}() is not available when Node.js runs the workflow; use setState()`);
  };
}

export const setStepOutput = unsupported("setStepOutput");
export const getStepOutput = unsupported("getStepOutput");
export const getOrSetStepOutput = unsupported("getOrSetStepOutput");

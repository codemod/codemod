// `codemod:workflow` in Node.js and Bun: the shared orchestrator, with the
// run looked up per call (see context.mjs).
//
// In the workflow body, `step` goes to the run's orchestrator. Workflows have
// no state: a step gets values from the body and returns what it found.

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

export function __evaluate(kind: string, expression: string, scope: unknown): unknown {
  return current("__evaluate()").runtime.__evaluate(kind, expression, scope);
}

// The dependency APIs need the engine's lockfile reader and registry client,
// which only the codemod CLI has. They are exported here so a workflow that
// imports them still loads, and fail with a clear message when used.
const QUICKJS_ONLY =
  'is not available on the Node.js host yet: run the workflow with the codemod CLI, or test it with { host: "quickjs" }';

function unavailable(what: string): never {
  throw new Error(`${what} ${QUICKJS_ONLY}`);
}

export function dependencyGraph(_options: unknown, _callback: unknown): never {
  return unavailable("dependencyGraph()");
}

export const registry = Object.freeze({
  npm: (_name: string): Promise<never> =>
    Promise.reject(new Error(`registry.npm() ${QUICKJS_ONLY}`)),
  cargo: (_name: string): Promise<never> =>
    Promise.reject(new Error(`registry.cargo() ${QUICKJS_ONLY}`)),
});

export const semver = Object.freeze({
  satisfies: (): never => unavailable("semver.satisfies()"),
  compare: (): never => unavailable("semver.compare()"),
  gt: (): never => unavailable("semver.gt()"),
  lt: (): never => unavailable("semver.lt()"),
  isBreaking: (): never => unavailable("semver.isBreaking()"),
  isPrerelease: (): never => unavailable("semver.isPrerelease()"),
});

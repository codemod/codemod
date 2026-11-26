// `codemod:workflow` in Node.js and Bun: the shared orchestrator, with the
// run looked up per call (see context.mjs).
//
// In the workflow body, `step` goes to the run's orchestrator. Workflows have
// no state: a step gets values from the body and returns what it found.

import { createHash } from "node:crypto";

import { current } from "./context.mts";
import { createTasks } from "./orchestrator.mts";
import type { Callback, Duration, Json, Runtime } from "./orchestrator.mts";

export {
  AwaitingApprovalError,
  CanceledError,
  StepError,
  WorkflowFailure,
  fail,
} from "./orchestrator.mts";

/** Identifies a callback given as a function, for replay checks. */
export function callbackId(fn: Callback): string {
  return `fn:${createHash("sha256").update(String(fn)).digest("hex").slice(0, 16)}`;
}

export const {
  __callback,
  __importedCallback,
  forEachFile,
  shell,
  http,
  astGrep,
  shard,
  codemod,
  agent,
  installSkill,
  approval,
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

/** @internal */
export function __evaluate(kind: string, expression: string, scope: unknown): unknown {
  return current("__evaluate()").runtime.__evaluate(kind, expression, scope);
}

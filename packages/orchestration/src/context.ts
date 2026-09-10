/**
 * The active workflow runtime. Author code never receives a context object:
 * a command created by calling a runnable finds the runtime that is executing
 * the current workflow body when it is awaited. The Node prototype carries
 * that binding with `AsyncLocalStorage`, which follows the body's async
 * continuations and is never a process-global. A production host (restricted
 * QuickJS) would bind the same runtime to the sandbox instance instead.
 */
import { AsyncLocalStorage } from "node:async_hooks";
import type { Command } from "./command.ts";

/** Internal runtime surface used by commands, plans, and parallel groups. */
export interface Runtime {
  /** Note that a command was created while this workflow body is active. */
  created(command: Command): void;
  /** Resolve a command (replay or execute). Memoized per command within one run. */
  issue<O>(command: Command<O>): Promise<O>;
}

const storage = new AsyncLocalStorage<Runtime>();

export function activeRuntime(): Runtime | undefined {
  return storage.getStore();
}

export function withRuntime<T>(runtime: Runtime, fn: () => T): T {
  return storage.run(runtime, fn);
}

export class NoActiveWorkflowError extends Error {
  constructor(what: string) {
    super(
      `${what} was awaited outside a workflow; await it inside workflow(async () => ...) or run it with run(plan(...))`,
    );
    this.name = "NoActiveWorkflowError";
  }
}

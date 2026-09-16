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

/** Internal runtime surface used by commands and composition nodes. */
export interface Runtime {
  /** Note that a command was created while this workflow body is active. */
  created(command: Command<unknown, unknown>): void;
  /** Note a static node built while this workflow body is active. */
  createdComposition(composition: object, commandIds: readonly string[]): void;
  /** Mark a static node as awaited or returned. */
  startedComposition(composition: object): void;
  /** Mark a command invocation as owned by a started static node. */
  claimed(command: Command<unknown, unknown>): void;
  /** Resolve a command (replay or execute). Memoized per command within one run. */
  issue<O>(
    command: Command<O, unknown>,
    concurrent?: boolean,
    flow?: { input: unknown },
  ): Promise<O>;
}

const storage = new AsyncLocalStorage<Runtime>();

export function activeRuntime(): Runtime | undefined {
  return storage.getStore();
}

export function withRuntime<T>(runtime: Runtime, fn: () => T): T {
  return storage.run(runtime, fn);
}

export class NoActiveRunError extends Error {
  constructor(what: string) {
    super(
      `${what} was awaited outside an active run; await it inside dynamic(async () => ...) or pass it to run(...)`,
    );
    this.name = "NoActiveRunError";
  }
}

/**
 * The active workflow runtime. Author code never receives a context object:
 * a command created by calling a runnable finds the runtime that is executing
 * the current workflow body when it is awaited.
 *
 * The binding is an `AsyncContextVariable` (the TC39 `AsyncContext.Variable`
 * shape) created from the installed host (`core/host.ts`): `AsyncLocalStorage`
 * on Node, promise-hook propagation in a QuickJS host. It follows the body's
 * async continuations and is never a process-global value. Static composition
 * passes runtimes explicitly (`runStage(runtime, ...)`); only commands and
 * composition nodes awaited by author code inside `dynamic()` (or by the root
 * body) rely on the variable.
 *
 * The variable is created once per JS realm, from the host installed when the
 * first run binds a runtime. Until then nothing is bound and `activeRuntime()`
 * is `undefined` without touching the host, so defining runnables and
 * commands at module top level needs no host.
 */
import { getHost, type AsyncContextVariable } from "../core/host.ts";
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
    dynamicId?: string,
  ): Promise<O>;
}

let variable: AsyncContextVariable<Runtime> | undefined;

export function activeRuntime(): Runtime | undefined {
  return variable?.get();
}

export function withRuntime<T>(runtime: Runtime, fn: () => T): T {
  variable ??= getHost().createAsyncContext<Runtime>();
  return variable.run(runtime, fn);
}

export class NoActiveRunError extends Error {
  constructor(what: string) {
    super(
      `${what} was awaited outside an active run; await it inside dynamic(async () => ...) or pass it to run(...)`,
    );
    this.name = "NoActiveRunError";
  }
}

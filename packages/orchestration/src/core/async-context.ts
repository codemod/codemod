/**
 * A stopgap `AsyncContextVariable` for hosts that cannot propagate context
 * across promise continuations yet (a QuickJS context without promise hooks).
 * It is NOT equivalent to `AsyncLocalStorage`; use it only until the host
 * provides real propagation (see `RUNTIMES.md`, "Async context").
 *
 * Semantics:
 * - inside the synchronous extent of `run(value, fn)`, `get()` is `value`
 *   (an ordinary stack, so nesting works while nothing awaits);
 * - anywhere else, `get()` is the value of the OLDEST `run` whose `fn`
 *   returned a promise that has not settled yet: for the orchestration
 *   runtime, the root run's runtime.
 *
 * What that loses, compared with real propagation: after its first `await`,
 * a `dynamic()` body nested in a static `parallel()` issues its commands to the
 * root runtime, so they are not marked `concurrent` (history content differs
 * from a Node run, and replay may report `reordered`), and commands issued
 * after an `await` inside any nested `dynamic()` lose their `dynamicId` in
 * events. Static composition is unaffected (it passes runtimes explicitly), as
 * are single-run sequential workflows. Two concurrent runs in one realm are
 * not separated; a QuickJS host runs exactly one.
 */
import type { AsyncContextVariable } from "./host.ts";

export function createFallbackAsyncContext<T>(): AsyncContextVariable<T> {
  const stack: T[] = [];
  const pending: { value: T }[] = [];
  return {
    get: () => (stack.length > 0 ? stack[stack.length - 1] : pending[0]?.value),
    run<R>(value: T, fn: () => R): R {
      stack.push(value);
      let result: R;
      try {
        result = fn();
      } finally {
        stack.pop();
      }
      if (
        typeof result === "object" &&
        result !== null &&
        typeof (result as { then?: unknown }).then === "function"
      ) {
        const entry = { value };
        pending.push(entry);
        const done = () => {
          const index = pending.indexOf(entry);
          if (index !== -1) pending.splice(index, 1);
        };
        (result as unknown as PromiseLike<unknown>).then(done, done);
      }
      return result;
    },
  };
}

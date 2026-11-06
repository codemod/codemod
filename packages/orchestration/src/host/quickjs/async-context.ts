/**
 * `AsyncContextVariable` with `AsyncLocalStorage` semantics for the QuickJS
 * host, by propagating a context frame through `Promise.prototype.then`.
 *
 * Why not promise hooks (the approach `RUNTIMES.md` first proposed): the
 * vendored quickjs-ng (`rquickjs-sys` 0.11) emits `JS_PROMISE_HOOK_BEFORE`/
 * `AFTER` only around thenable-resolution jobs, not around ordinary promise
 * reaction jobs, and `await` registers its continuation without a derived
 * promise (`js_async_function_resume` -> `perform_promise_then` with no
 * capability). A hook-based variable could not see an `await` resume.
 *
 * What works instead, exactly: the bundle is built with async functions and
 * async generators lowered to generators (`build: supported: { "async-await":
 * false, ... }` in `host/quickjs-bundle.ts`), so every continuation in the
 * bundle is registered through `Promise.prototype.then` (esbuild's `__async`
 * helper does `Promise.resolve(x).then(fulfilled, rejected)`; `Promise.all`,
 * `finally`, and thenable adoption invoke `then` too). `then` is patched once
 * per realm to capture the current frame at registration and restore it
 * around the callback: the same "context at the time the continuation was
 * registered" rule V8 implements for `AsyncLocalStorage`. Nested `dynamic()`
 * inside `parallel()` keeps `concurrent: true` and `dynamicId`s match Node.
 *
 * Limits: code that is not lowered (transform artifacts, evaluated as
 * shipped) awaits natively and runs its continuations with no frame; nothing
 * inside a transform reads the orchestration context, and a command awaited
 * inside a transform then fails with `NoActiveRunError`, as intended. A frame
 * is an immutable `Map` keyed by variable, so `run()` is O(variables).
 */
import type { AsyncContextVariable } from "../../core/host.ts";

type Frame = ReadonlyMap<object, unknown> | undefined;
type Then = (this: unknown, onFulfilled?: unknown, onRejected?: unknown) => unknown;

let current: Frame;
let installed = false;
let adopt: (value: unknown, frame: Frame) => unknown = (value) => value;

function bind(callback: unknown, frame: Frame): unknown {
  if (typeof callback !== "function") return callback;
  const fn = callback as (this: unknown, value: unknown) => unknown;
  return function (this: unknown, value: unknown): unknown {
    const previous = current;
    current = frame;
    let result: unknown;
    try {
      result = fn.call(this, value);
    } finally {
      current = previous;
    }
    // A thenable returned from a callback is adopted in a later job; run its
    // `then` in this callback's frame.
    return adopt(result, frame);
  };
}

/**
 * Patch `Promise.prototype.then` and `Promise.resolve` of this realm and
 * replace `globalThis.Promise` with a construct-wrapping `Proxy` of it
 * (idempotent). Must run before any continuation that should observe a
 * context is registered; the bundle entry calls it while the bundle
 * evaluates.
 *
 * Thenables that are not native promises (orchestration `Command`s) are
 * adopted by QuickJS in a "resolve thenable" job that calls their `then`
 * outside any frame. Node runs that job in the frame where the promise was
 * resolved, and `Command.then` reads the run from the context, so adoption
 * is made context-aware everywhere a promise is resolved with a thenable:
 * `Promise.resolve(thenable)` (what lowered `await` does, and what
 * `Promise.all` calls), a thenable returned from a `then` callback, and an
 * executor's `resolve(thenable)` (what a lowered async function's `return`
 * does). The thenable's `then` runs one job later in the frame of the
 * resolution, as in Node. `instanceof Promise` still holds for every
 * promise (the proxy forwards `prototype`).
 */
export function installPromiseContextPropagation(promise: PromiseConstructor = Promise): void {
  if (installed) return;
  installed = true;
  const realm = globalThis as { Promise: PromiseConstructor };
  let proxy: PromiseConstructor | undefined;
  if (realm.Promise === promise) {
    proxy = realm.Promise = new Proxy(promise, {
      construct(target, args, newTarget) {
        const executor = args[0] as unknown;
        if (typeof executor !== "function") return Reflect.construct(target, args, newTarget);
        const run = executor as (
          resolve: (value: unknown) => void,
          reject: (reason: unknown) => void,
        ) => void;
        return Reflect.construct(
          target,
          [
            (resolve: (value: unknown) => void, reject: (reason: unknown) => void) =>
              run((value) => resolve(adopt(value, current)), reject),
          ],
          newTarget,
        );
      },
    });
  }
  const then = promise.prototype.then as Then;
  const resolve = promise.resolve.bind(promise) as (value?: unknown) => Promise<unknown>;
  adopt = (value, frame) => {
    if ((typeof value !== "object" && typeof value !== "function") || value === null) return value;
    if (value instanceof promise) return value;
    let thenable: unknown;
    try {
      thenable = (value as { then?: unknown }).then;
    } catch (error) {
      return new promise((_, reject) => reject(error));
    }
    if (typeof thenable !== "function") return value;
    return new promise((onResolve, onReject) => {
      then.call(resolve(), () => {
        const previous = current;
        current = frame;
        try {
          (thenable as Then).call(value, onResolve, onReject);
        } catch (error) {
          onReject(error);
        } finally {
          current = previous;
        }
      });
    });
  };
  // Patching `then` is the point: it carries the context.
  // oxlint-disable-next-line unicorn/no-thenable
  Object.defineProperty(promise.prototype, "then", {
    configurable: true,
    writable: true,
    value: function (this: unknown, onFulfilled?: unknown, onRejected?: unknown): unknown {
      const frame = current;
      return then.call(this, bind(onFulfilled, frame), bind(onRejected, frame));
    },
  });
  Object.defineProperty(promise, "resolve", {
    configurable: true,
    writable: true,
    value: function (this: unknown, value?: unknown): unknown {
      return resolve(this === promise || this === proxy ? adopt(value, current) : value);
    },
  });
}

/** A variable over the propagated frame. Requires `installPromiseContextPropagation()`. */
export function createPropagatingAsyncContext<T>(): AsyncContextVariable<T> {
  const key = {};
  return {
    get: () => current?.get(key) as T | undefined,
    run<R>(value: T, fn: () => R): R {
      const previous = current;
      const next = new Map(previous);
      next.set(key, value);
      current = next;
      try {
        return fn();
      } finally {
        current = previous;
      }
    },
  };
}

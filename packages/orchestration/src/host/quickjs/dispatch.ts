/**
 * In-context JSSG for the `quickjs` runtime: the `JssgBatchEngine` the
 * portable batch loop (`execution/jssg-batch.ts`, shared with the `node`
 * runtime) drives instead of spawning a bridge process. Every batch rule is
 * the bridge's own Rust (`crates/jssg-batch`), reached through the host's
 * primitives (`crates/orchestration-quickjs-host/src/jssg.rs`):
 *
 * - `jssgBegin(config, paths)`: `BatchPlan` (hash check, containment of
 *   every path, selector, params; semantic analysis refused);
 * - `jssgParse(batch, start, contents)`: roots, `null` where the selector
 *   skips (on this thread: QuickJS values cannot cross threads);
 * - `jssgCall(batch)` / `jssgCallEnd(batch, index)`: around one transform
 *   call, the per-context staging of `write()`/`jssgTransform()` and the
 *   transform options;
 * - `jssgFinish(batch, index, root, result)`: the bridge's result
 *   conversion, as `FileOutcome` JSON.
 *
 * Transform modules are evaluated once per run, cached by artifact hash;
 * module-level state is therefore shared by every file and command of the
 * run and is unsupported. Transform calls run one at a time across the
 * whole run (the staging is per context); parsing, selection, reads,
 * staging, and commits of concurrent commands still interleave.
 * Runs inside QuickJS: Node-free (`tests/portable.test.ts`).
 */
import type { FileOutcome } from "../../core/protocol.ts";
import {
  createJssgBatchDispatch,
  type JssgBatchEngine,
  type TransformFunction,
} from "../../execution/jssg-batch.ts";
import type { JssgDispatch } from "../../execution/jssg.ts";
import type { QuickjsNative } from "./native.ts";

/** Accumulated wall time inside dispatch, for the timer breakdown. */
export interface DispatchTimings {
  /** Module registration and evaluation (once per artifact). */
  loadMs: number;
  /** Time inside transform functions (including awaits). */
  transformMs: number;
  batches: number;
}

export interface QuickjsDispatch {
  dispatch: JssgDispatch;
  timings: DispatchTimings;
}

/** `Name: message` plus the stack (QuickJS's `stack` holds only the frames). */
const describe = (error: unknown) => {
  if (!(error instanceof Error)) return String(error);
  const head = `${error.name}: ${error.message}`;
  const stack = error.stack?.trimEnd();
  if (stack === undefined || stack === "") return head;
  return stack.startsWith(head) ? stack : `${head}\n${stack}`;
};

export function createQuickjsDispatch(native: QuickjsNative): QuickjsDispatch {
  const timings: DispatchTimings = { loadMs: 0, transformMs: 0, batches: 0 };
  let queue: Promise<unknown> = Promise.resolve();
  /** Run `task` once every earlier transform call has settled. */
  const serialized = <T>(task: () => Promise<T>): Promise<T> => {
    const next = queue.then(task);
    queue = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  };

  const engine: JssgBatchEngine<unknown, unknown> = {
    open(config, paths) {
      timings.batches += 1;
      const { id, module } = native.jssgBegin(JSON.stringify(config), paths);
      return {
        module,
        parse: (start, contents) => Promise.resolve(native.jssgParse(id, start, contents)),
        call: (index, root, transform: TransformFunction) =>
          serialized(async () => {
            const options = native.jssgCall(id);
            try {
              return await transform(root, options);
            } finally {
              native.jssgCallEnd(id, index);
            }
          }),
        finish: (index, root, result) =>
          JSON.parse(native.jssgFinish(id, index, root, result)) as FileOutcome,
        close: () => native.jssgEnd(id),
      };
    },
    load(artifact, module) {
      const specifier = `jssg-artifact:${artifact.hash}/${module}`;
      native.registerModule(specifier, artifact.source);
      return (import(specifier) as Promise<{ default?: unknown }>).then(
        (namespace) => namespace.default,
      );
    },
    describeError: describe,
  };

  const dispatch = createJssgBatchDispatch({
    engine,
    now: () => native.hrtime(),
    profile: (phase, ms) => {
      if (phase === "load") timings.loadMs += ms;
      else if (phase === "transform") timings.transformMs += ms;
    },
  });
  return { dispatch, timings };
}

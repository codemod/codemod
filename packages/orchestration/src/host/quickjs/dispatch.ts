/**
 * In-context JSSG for the `quickjs` runtime: the `JssgBatchEngine` the
 * portable batch loop (`execution/jssg-batch.ts`, shared with the `node`
 * runtime) drives. Every batch rule is the shared Rust
 * (`crates/jssg-batch`), reached through the host's
 * primitives (`crates/orchestration-quickjs-host/src/jssg.rs`):
 *
 * - `jssgBegin(config, paths)`: `BatchPlan` (hash check, containment of
 *   every path, selector, params, semantic provider);
 * - `jssgIndex(batch, start, contents)`: the workspace index (workspace
 *   semantic analysis, before the first transform);
 * - `jssgParse(batch, start, contents)`: roots, `null` where the selector
 *   skips (on this thread: QuickJS values cannot cross threads);
 * - `jssgCall(batch)` / `jssgCallEnd(batch, index)`: around one transform
 *   call, the per-context staging of `write()`/`jssgTransform()` and the
 *   transform options;
 * - `jssgFinish(batch, index, root, result)`: the shared result
 *   conversion, as `FileOutcome` JSON;
 * - `jssgTransformRange(...)`: batches of `POOL_MIN_FILES` or more run in
 *   chunks on the host's transform pool, one QuickJS context per thread.
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
  type RangeResult,
  type TransformFunction,
} from "../../execution/jssg-batch.ts";
import type { JssgDispatch } from "../../execution/jssg.ts";
import type { QuickjsNative } from "./native.ts";

/** `Name: message` plus the stack (QuickJS's `stack` holds only the frames). */
/** Batches at least this large run on the transform pool; smaller ones stay in this context. */
const POOL_MIN_FILES = 64;

const specifierOf = (hash: string, module: string) => `jssg-artifact:${hash}/${module}`;

const describe = (error: unknown) => {
  if (!(error instanceof Error)) return String(error);
  const head = `${error.name}: ${error.message}`;
  const stack = error.stack?.trimEnd();
  if (stack === undefined || stack === "") return head;
  return stack.startsWith(head) ? stack : `${head}\n${stack}`;
};

/**
 * Benchmark timings: the host times `jssgBegin` (setup) in Rust; the
 * shared loop times the chunk waits (parse: the yield, then `jssgParse`),
 * the loop's per-file work and the first module evaluation (setup), the
 * transform calls, and `finish` (convert).
 */
export function createQuickjsDispatch(native: QuickjsNative): JssgDispatch {
  // One transform call in flight across the run (the staging is per
  // context). Without contention a call starts at once, and a synchronous
  // transform settles without extra promise hops: in the bundle every
  // `then` also carries the async-context frame, so hops are not free.
  let busy = false;
  const waiting: (() => void)[] = [];
  const release = () => {
    const next = waiting.shift();
    if (next === undefined) busy = false;
    else next();
  };
  const exclusive = <T>(start: () => Promise<T>): Promise<T> => {
    if (!busy) {
      busy = true;
      return start();
    }
    return new Promise<void>((resolve) => waiting.push(resolve)).then(start);
  };

  const engine: JssgBatchEngine<unknown, unknown> = {
    open(config, paths) {
      const { id, module, indexesWorkspace } = native.jssgBegin(JSON.stringify(config), paths);
      const workers = paths.length >= POOL_MIN_FILES ? native.jssgPoolSize() : 1;
      const end = (index: number) => {
        native.jssgCallEnd(id, index);
        release();
      };
      return {
        ...(workers > 1 && {
          parallelism: workers,
          transformRange: (start: number, contents: string[], artifact) =>
            native
              .jssgTransformRange(
                id,
                specifierOf(artifact.hash, module),
                artifact.source,
                start,
                paths.slice(start, start + contents.length),
                contents,
              )
              .then((text) => (JSON.parse(text) as { results: RangeResult[] }).results),
        }),
        module,
        indexesWorkspace,
        index: (start, contents) =>
          native.yieldNow().then(() => native.jssgIndex(id, start, contents)),
        // Yield to the host's event loop once per chunk (64 files) so a
        // SIGINT, child output, and timers are serviced mid-batch; the loop
        // checks the abort signal before every file.
        parse: (start, contents) =>
          native.yieldNow().then(() => native.jssgParse(id, start, contents)),
        call: (index, root, transform: TransformFunction) =>
          exclusive(() => {
            let value: unknown;
            try {
              value = transform(root, native.jssgCall(id));
            } catch (error) {
              end(index);
              return Promise.reject(error);
            }
            if (typeof (value as PromiseLike<unknown> | null)?.then !== "function") {
              end(index);
              return Promise.resolve(value);
            }
            return Promise.resolve(value).then(
              (result) => {
                end(index);
                return result;
              },
              (error: unknown) => {
                end(index);
                throw error;
              },
            );
          }),
        finish: (index, root, result) => {
          const outcome = native.jssgFinish(id, index, root, result);
          return outcome === undefined
            ? { path: paths[index]!, edits: [] }
            : (JSON.parse(outcome) as FileOutcome);
        },
        close: () => native.jssgEnd(id),
      };
    },
    load(artifact, module) {
      const specifier = specifierOf(artifact.hash, module);
      native.registerModule(specifier, artifact.source);
      return (import(specifier) as Promise<{ default?: unknown }>).then(
        (namespace) => namespace.default,
      );
    },
    describeError: describe,
  };

  return createJssgBatchDispatch({ engine });
}

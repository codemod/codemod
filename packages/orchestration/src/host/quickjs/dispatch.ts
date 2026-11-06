/**
 * In-context JSSG dispatch for the `quickjs` runtime: the `JssgDispatch`
 * `executeJssgCommand` calls instead of spawning a bridge process. Same
 * contract as the bridge's batch (`crates/execution-bridge/src/jssg.rs`):
 * one `FileOutcome` per file in request order, snapshot semantics (every
 * transform sees only the content `executeJssgCommand` read), nothing
 * written, files the static selector does not match skipped before the
 * transform module is even loaded.
 *
 * Per batch the Rust host plans it with the bridge's own `BatchPlan` (hash
 * check, containment of every path, selector, params; semantic analysis is
 * refused). Per file: `jssgPrepare` (Rust: selector check, parse into an
 * `SgRoot`, transform options), the transform call (here, on the one JS
 * thread), `jssgFinish` (Rust: the bridge's result conversion). Transform
 * modules are evaluated once per run, cached by artifact hash; module-level
 * state is therefore shared by every file and command of the run and is
 * unsupported.
 *
 * Batches run one at a time across the whole run (a global queue): the
 * host's per-file state assumes one file in flight, and there is only one JS
 * thread to run transforms on anyway. Selection, reads, staging, and commits
 * of concurrent commands still interleave in `executeJssgCommand`.
 * Runs inside QuickJS: Node-free (`tests/portable.test.ts`).
 */
import type { Json } from "../../core/json.ts";
import type { JssgDispatch } from "../../execution/jssg.ts";
import {
  PROTOCOL_VERSION,
  type FileOutcome,
  type JssgOperation,
  type OperationCompletion,
  type OperationRequest,
} from "../../core/protocol.ts";
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
const message = (error: unknown) => {
  if (!(error instanceof Error)) return String(error);
  const head = `${error.name}: ${error.message}`;
  const stack = error.stack?.trimEnd();
  if (stack === undefined || stack === "") return head;
  return stack.startsWith(head) ? stack : `${head}\n${stack}`;
};

export function createQuickjsDispatch(native: QuickjsNative): QuickjsDispatch {
  const modules = new Map<string, Promise<unknown>>();
  const timings: DispatchTimings = { loadMs: 0, transformMs: 0, batches: 0 };
  let queue: Promise<unknown> = Promise.resolve();

  const load = (hash: string, module: string, source: string): Promise<unknown> => {
    let loaded = modules.get(hash);
    if (loaded === undefined) {
      const specifier = `jssg-artifact:${hash}/${module}`;
      const started = native.hrtime();
      native.registerModule(specifier, source);
      loaded = (import(specifier) as Promise<{ default?: unknown }>).then((namespace) => {
        timings.loadMs += native.hrtime() - started;
        return namespace.default;
      });
      modules.set(hash, loaded);
    }
    return loaded;
  };

  const runBatch = async (
    request: OperationRequest,
    signal: AbortSignal | undefined,
  ): Promise<OperationCompletion> => {
    const fail = (text: string): OperationCompletion => ({
      protocolVersion: PROTOCOL_VERSION,
      commandId: request.commandId,
      status: "failed",
      error: { message: text },
    });
    const operation = request.operation as JssgOperation;
    const files = request.context?.files ?? [];
    const source = request.context?.artifact?.source;
    timings.batches += 1;
    let batch: { id: number; module: string };
    try {
      batch = native.jssgBegin(
        JSON.stringify({
          transform: operation.transform,
          source,
          language: operation.language,
          selector: operation.selector,
          semanticAnalysis: operation.semanticAnalysis,
          input: operation.input,
          targetRoot: request.context?.targetRoot,
        }),
        files.map((file) => file.path),
      );
    } catch (error) {
      return fail((error as Error).message ?? String(error));
    }
    try {
      const outcomes: FileOutcome[] = [];
      for (const [index, file] of files.entries()) {
        if (signal?.aborted) return fail("cancelled");
        const prepared = native.jssgPrepare(batch.id, index, file.content);
        if (prepared === null) {
          outcomes.push({ path: file.path, edits: [] });
          continue;
        }
        const started = native.hrtime();
        let result: unknown;
        try {
          const transform = await load(operation.transform.hash, batch.module, source ?? "");
          if (typeof transform !== "function") {
            throw new Error(
              `jssg codemod modules must export a function. ${typeof transform} found.`,
            );
          }
          result = await (transform as (root: unknown, options: unknown) => unknown)(
            prepared[0],
            prepared[1],
          );
        } catch (error) {
          return fail(`JSSG failed for '${file.path}': ${message(error)}`);
        } finally {
          timings.transformMs += native.hrtime() - started;
        }
        outcomes.push(JSON.parse(native.jssgFinish(batch.id, index, result)) as FileOutcome);
      }
      return {
        protocolVersion: PROTOCOL_VERSION,
        commandId: request.commandId,
        status: "succeeded",
        output: { files: outcomes as unknown as Json[] },
      };
    } catch (error) {
      return fail((error as Error).message ?? String(error));
    } finally {
      native.jssgEnd(batch.id);
    }
  };

  const dispatch: JssgDispatch = (request, signal) => {
    const next = queue.then(() => runBatch(request, signal));
    queue = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  };
  return { dispatch, timings };
}

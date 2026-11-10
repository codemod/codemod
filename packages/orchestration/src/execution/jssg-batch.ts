/**
 * The JSSG batch loop of the `quickjs` runtime: a
 * `JssgDispatch` that runs transforms in the same JS realm as the control
 * plane. Every rule of the batch lives in Rust, in one place
 * (`crates/orchestration/src/jssg_batch.rs`), reached through the engine:
 *
 * - `open` plans the batch (`BatchPlan`: hash check, canonical target root,
 *   every path contained in it, language, compiled selector, params, the
 *   semantic provider) and fails before anything runs;
 * - `index` builds the workspace index from every file's content before the
 *   first transform (workspace semantic analysis only), so a transform can
 *   resolve definitions in files the selector skips;
 * - `parse` parses files from the content the host sent (snapshot
 *   semantics: no transform sees another's edits) and answers `null` for
 *   files the static selector does not match (`{ path, edits: [] }`), so a
 *   skipped file never reaches JavaScript (large batches go to the host's
 *   transform pool instead, `host/quickjs/dispatch.ts`);
 * - `call` runs one transform as the only one in flight in the realm, with
 *   `write()` / `jssgTransform()` staged (never written) and contained to
 *   the target root, and the options the sandbox builds;
 * - `finish` converts the result with the shared rules (string / null /
 *   `{ content, output }`, `rename()`, staged edits, root-relative `/` paths
 *   after a containment check) and names the file in any error.
 *
 * What stays here is only the loop: chunking, cancellation between files,
 * evaluating each artifact module once per dispatcher (one per run), cached
 * by hash and only when a file reaches a transform, and turning failures
 * into a `failed` completion. It never writes the repository and never
 * throws; `executeJssgCommand` commits nothing on failure.
 *
 * Portable (`tests/portable.test.ts`): nothing here imports Node. A
 * transform that never returns cannot be interrupted in-process.
 */
import type { Json } from "../core/json.ts";
import {
  PROTOCOL_VERSION,
  type ArtifactRef,
  type FileOutcome,
  type JssgOperation,
  type OperationCompletion,
  type OperationRequest,
} from "../core/protocol.ts";
import type { JssgDispatch } from "./jssg.ts";

/** An artifact as the request carries it plus its identity. */
export interface TransformArtifact extends ArtifactRef {
  source: string;
}

/** The batch as Rust plans it (`jssg_batch::BatchConfig`); the paths travel separately. */
export interface JssgBatchConfig {
  transform: ArtifactRef;
  source: string | undefined;
  language: string;
  selector: JssgOperation["selector"];
  semanticAnalysis: JssgOperation["semanticAnalysis"];
  input: JssgOperation["input"];
  targetRoot: string | undefined;
}

/** A transform module's default export. */
export type TransformFunction = (root: unknown, options: unknown) => unknown;

/** One planned batch (a Rust `BatchPlan` behind a binding). */
export interface JssgBatch<Root = unknown, Ran = unknown> {
  /** Virtual module name of the artifact (`<name>.jssg.js`). */
  readonly module: string;
  /** Whether every file must go through `index` before the first transform (workspace semantic analysis). */
  readonly indexesWorkspace: boolean;
  /** Add files `start ..< start + contents.length` to the workspace index. */
  index(start: number, contents: string[]): Promise<void>;
  /** Roots of files `start ..< start + contents.length`; `null` where the selector skips. */
  parse(start: number, contents: string[]): Promise<(Root | null)[]>;
  /**
   * Call `transform(root, options)` for file `index` as the only transform
   * in flight in this realm, staging `write()`/`jssgTransform()`. Resolves
   * with what `finish` needs; rejects with what the transform threw.
   */
  call(index: number, root: Root, transform: TransformFunction): Promise<Ran>;
  /** The file's `FileOutcome`; throws `JSSG failed for '<path>': ...`. */
  finish(index: number, root: Root, ran: Ran): FileOutcome;
  /**
   * Parse and transform files `start ..< start + contents.length` on another
   * thread, one result per file in order. Present when the engine runs
   * transforms on a worker pool of `parallelism` threads; the loop then
   * hands it every chunk at once and the pool runs them side by side.
   */
  transformRange?(
    start: number,
    contents: string[],
    artifact: TransformArtifact,
  ): Promise<RangeResult[]>;
  /** Worker threads behind `transformRange`. */
  readonly parallelism?: number;
  /** Release the batch. */
  close(): void;
}

/** One file of a `transformRange`: its outcome, or the `JSSG failed for '<path>': ...` message. */
export type RangeResult = { outcome: FileOutcome } | { error: string };

/** What a runtime provides to run JSSG batches in-process. */
export interface JssgBatchEngine<Root = unknown, Ran = unknown> {
  /** Plan a batch; throws with the planning error (nothing has run). */
  open(config: JssgBatchConfig, paths: string[]): JssgBatch<Root, Ran>;
  /** Evaluate an artifact module and return its default export (called once per hash per dispatcher). */
  load(artifact: TransformArtifact, module: string): Promise<unknown>;
  /** How a thrown transform error reads after `JSSG failed for '<path>': `. */
  describeError(error: unknown): string;
}

export interface JssgBatchDispatchOptions {
  engine: JssgBatchEngine<any, any>;
  /** Files parsed per `parse` call; the next chunk is requested before this one transforms. Default 64. */
  chunkSize?: number;
}

/** `Name: message` (what the sandbox's error shows for a thrown `Error`). */
export function describeError(error: unknown): string {
  if (error instanceof Error) return `${error.name}: ${error.message}`;
  return String(error);
}

/** A `JssgDispatch` running transforms in this realm; create one per run. */
export function createJssgBatchDispatch(options: JssgBatchDispatchOptions): JssgDispatch {
  const { engine } = options;
  const chunkSize = Math.max(1, options.chunkSize ?? 64);
  const modules = new Map<string, Promise<unknown>>();
  const load = (artifact: TransformArtifact, module: string) => {
    let loaded = modules.get(artifact.hash);
    if (loaded === undefined) {
      loaded = engine.load(artifact, module);
      modules.set(artifact.hash, loaded);
    }
    return loaded;
  };

  return async (request: OperationRequest, signal?: AbortSignal) => {
    const failed = (message: string): OperationCompletion => ({
      protocolVersion: PROTOCOL_VERSION,
      commandId: request.commandId,
      status: signal?.aborted ? "cancelled" : "failed",
      error: { message },
    });
    const operation = request.operation as JssgOperation;
    if (operation.kind !== "jssg") return failed(`not a jssg operation: ${operation.kind}`);
    const files = request.context?.files ?? [];
    const source = request.context?.artifact?.source;
    let batch: JssgBatch;
    try {
      batch = engine.open(
        {
          transform: operation.transform,
          source,
          language: operation.language,
          selector: operation.selector,
          semanticAnalysis: operation.semanticAnalysis,
          input: operation.input,
          targetRoot: request.context?.targetRoot,
        },
        files.map((file) => file.path),
      );
    } catch (error) {
      return failed((error as Error).message ?? String(error));
    }
    const artifact: TransformArtifact = { ...operation.transform, source: source ?? "" };
    const skipFailures = (operation.onFileError ?? "skip") === "skip";
    const outcomes: FileOutcome[] = [];
    const failures: { path: string; error: string }[] = [];
    /** Record a failed file; false when the policy fails the command instead. */
    const fileFailed = (path: string, error: string): boolean => {
      if (!skipFailures) return false;
      failures.push({ path, error });
      outcomes.push({ path, edits: [] });
      return true;
    };
    const succeeded = (): OperationCompletion => ({
      protocolVersion: PROTOCOL_VERSION,
      commandId: request.commandId,
      status: "succeeded",
      output: { files: outcomes, failures } as unknown as Json,
    });
    let pending: Promise<unknown[]> | undefined;
    try {
      if (batch.indexesWorkspace) {
        for (let start = 0; start < files.length; start += chunkSize) {
          if (signal?.aborted) return failed("cancelled");
          await batch.index(
            start,
            files.slice(start, start + chunkSize).map((file) => file.content),
          );
        }
      }
      const transformRange = batch.transformRange?.bind(batch);
      if (transformRange !== undefined) {
        // Small chunks keep every worker busy until the end of the batch.
        const size = Math.max(
          1,
          Math.min(chunkSize, Math.ceil(files.length / ((batch.parallelism ?? 1) * 4))),
        );
        const ranges: Promise<RangeResult[]>[] = [];
        for (let start = 0; start < files.length; start += size) {
          const contents = files.slice(start, start + size).map((file) => file.content);
          ranges.push(transformRange(start, contents, artifact));
        }
        for (const range of ranges) range.catch(() => {});
        for (const range of ranges) {
          const results = await range;
          if (signal?.aborted) return failed("cancelled");
          for (const result of results) {
            if ("outcome" in result) outcomes.push(result.outcome);
            else if (!fileFailed(files[outcomes.length]!.path, result.error)) {
              return failed(result.error);
            }
          }
        }
        return succeeded();
      }
      const parse = (start: number) =>
        batch.parse(
          start,
          files.slice(start, start + chunkSize).map((file) => file.content),
        );
      pending = files.length > 0 ? parse(0) : undefined;
      for (let start = 0; pending !== undefined; start += chunkSize) {
        const roots = await pending;
        // Parse the next chunk (off-thread, where the engine can) while this one transforms.
        pending = start + chunkSize < files.length ? parse(start + chunkSize) : undefined;
        for (const [offset, root] of roots.entries()) {
          if (signal?.aborted) return failed("cancelled");
          const index = start + offset;
          const file = files[index]!;
          if (root === null) {
            outcomes.push({ path: file.path, edits: [] });
            continue;
          }
          // A module that does not load fails every file alike, so it fails the command.
          let transform: unknown;
          try {
            transform = await load(artifact, batch.module);
            if (typeof transform !== "function") {
              throw new Error(
                `jssg codemod modules must export a function. ${typeof transform} found.`,
              );
            }
          } catch (error) {
            return failed(`JSSG failed for '${file.path}': ${engine.describeError(error)}`);
          }
          let outcome: FileOutcome;
          try {
            let ran: unknown;
            try {
              ran = await batch.call(index, root, transform as TransformFunction);
            } catch (error) {
              throw new Error(`JSSG failed for '${file.path}': ${engine.describeError(error)}`);
            }
            outcome = batch.finish(index, root, ran);
          } catch (error) {
            const message = (error as Error).message ?? String(error);
            if (!fileFailed(file.path, message)) return failed(message);
            continue;
          }
          outcomes.push(outcome);
        }
      }
      return succeeded();
    } catch (error) {
      return failed((error as Error).message ?? String(error));
    } finally {
      pending?.catch(() => {});
      batch.close();
    }
  };
}

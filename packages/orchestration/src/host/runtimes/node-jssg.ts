/**
 * The `JssgBatchEngine` of the `node` runtime, over the napi addon
 * (`@codemod.com/jssg-napi`, built from `crates/jssg-napi`):
 *
 * - batches are the shared Rust rules (`JssgBatch`, over
 *   `crates/jssg-batch`): planning, containment, the selector skip, parsing
 *   on worker threads, and result conversion;
 * - artifacts are evaluated as ES modules in this process
 *   (`@codemod.com/jssg-napi/loader`: in-memory module hooks,
 *   `codemod:ast-grep` resolved to the addon-backed module);
 * - transform calls are serialized per thread with staged writes
 *   (`runTransform`); batches of `POOL_MIN_FILES` or more run on a pool of
 *   worker threads (`@codemod.com/jssg-napi/pool`, `CODEMOD_JSSG_WORKERS`).
 */
import {
  loadBinding,
  runTransform,
  type NativeRoot,
  type StagedChanges,
} from "@codemod.com/jssg-napi";
import { importArtifact } from "@codemod.com/jssg-napi/loader";
import { createTransformPool, type TransformPool } from "@codemod.com/jssg-napi/pool";
import type { FileOutcome } from "../../core/protocol.ts";
import { addTimingsSource, timingsEnabled, type PhaseTotals } from "../../core/timings.ts";
import {
  describeError,
  type JssgBatchEngine,
  type RangeResult,
  type TransformArtifact,
} from "../../execution/jssg-batch.ts";

import type { Host } from "../../core/host.ts";

/**
 * `base` (the Node host) plus the addon's native file selection
 * (`Host.selectFiles`, `Host.fs.readSources`, `crates/orchestration-walker`):
 * the same files, contents, and error messages as the TypeScript walker
 * over `base` (`tests/files-native.test.ts`), without interpreting the walk.
 */
export function createNapiSelectHost(base: Host): Host {
  const native = loadBinding();
  return {
    ...base,
    fs: {
      ...base.fs,
      // Verbatim, a leading BOM included, as `readText(fatal)` (`Host.fs.readText`).
      readSources: (root, paths) =>
        native.readSources(root, [...paths]).map((content) => content ?? undefined),
    },
    selectFiles: (request) => native.selectFiles(JSON.stringify(request)),
  };
}

let engines = 0;
let timingsSourceAdded = false;
let pool: TransformPool | undefined;

/** Batches at least this large run on the transform pool; smaller ones stay on this thread. */
const POOL_MIN_FILES = 64;

/** One engine per run: each artifact is evaluated once per engine. */
export function createNapiJssgEngine(): JssgBatchEngine<
  NativeRoot,
  { result: unknown; staged: StagedChanges | null }
> {
  const native = loadBinding();
  const runId = `r${++engines}`;
  // Benchmark timings: the addon's phases (process-wide) join the CLI's file
  // once, when this build of the addon records them (older builds lack it).
  if (!timingsSourceAdded && timingsEnabled() && native.timingsEnabled?.() === true) {
    timingsSourceAdded = true;
    addTimingsSource(() => JSON.parse(native.timingsPhases()) as PhaseTotals);
  }
  return {
    open(config, paths) {
      const batch = new native.JssgBatch(JSON.stringify(config), paths);
      const { targetRoot, params } = batch;
      // The options Rust builds, once per batch; every file gets its own copy.
      const options = batch.options();
      pool ??= createTransformPool();
      const shared = pool.size > 1 && paths.length >= POOL_MIN_FILES ? batch.share() : undefined;
      const workers = pool;
      return {
        ...(shared === undefined
          ? {}
          : {
              parallelism: workers.size,
              transformRange: (start: number, contents: string[], artifact: TransformArtifact) =>
                workers.run({
                  batchId: shared,
                  runId,
                  artifact,
                  start,
                  paths: paths.slice(start, start + contents.length),
                  contents,
                }) as Promise<RangeResult[]>,
            }),
        module: batch.module,
        indexesWorkspace: batch.indexesWorkspace,
        index: (start, contents) => batch.index(start, contents),
        parse: (start, contents) => batch.parse(start, contents),
        call: (_index, root, transform) =>
          runTransform(targetRoot, params, () =>
            transform(root, { ...options, params: { ...options.params } }),
          ),
        finish: (index, root, ran) =>
          (batch.finish(index, root, ran.result, ran.staged) as FileOutcome | null) ?? {
            path: paths[index]!,
            edits: [],
          },
        close: () => {
          if (shared !== undefined) native.JssgBatch.unshare(shared);
        },
      };
    },
    load: (artifact) => importArtifact(artifact, runId),
    describeError,
  };
}

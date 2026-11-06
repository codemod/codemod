/**
 * The `JssgBatchEngine` of the `node` runtime, over the napi addon
 * (`@codemod.com/jssg-napi`, built from `crates/jssg-napi`):
 *
 * - batches are the bridge's own Rust rules (`JssgBatch`, over
 *   `crates/jssg-batch`): planning, containment, the selector skip, parsing
 *   on worker threads, and result conversion;
 * - artifacts are evaluated as ES modules in this process
 *   (`@codemod.com/jssg-napi/loader`: in-memory module hooks,
 *   `codemod:ast-grep` resolved to the addon-backed module);
 * - transform calls are serialized process-wide with staged writes
 *   (`runTransform`).
 */
import {
  loadBinding,
  runTransform,
  type NativeRoot,
  type StagedChanges,
} from "@codemod.com/jssg-napi";
import { importArtifact } from "@codemod.com/jssg-napi/loader";
import type { FileOutcome } from "../../core/protocol.ts";
import { describeError, type JssgBatchEngine } from "../../execution/jssg-batch.ts";

let engines = 0;

/** One engine per run: each artifact is evaluated once per engine. */
export function createNapiJssgEngine(): JssgBatchEngine<
  NativeRoot,
  { result: unknown; staged: StagedChanges | null }
> {
  const native = loadBinding();
  const runId = `r${++engines}`;
  return {
    open(config, paths) {
      const batch = new native.JssgBatch(JSON.stringify(config), paths);
      const { targetRoot, params } = batch;
      // The options Rust builds, once per batch; every file gets its own copy.
      const options = batch.options();
      return {
        module: batch.module,
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
        close: () => {},
      };
    },
    load: (artifact) => importArtifact(artifact, runId),
    describeError,
  };
}

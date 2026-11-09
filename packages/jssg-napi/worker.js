/**
 * A transform pool worker (`./pool.js`): runs whole chunks of a batch that
 * the main thread planned and published (`JssgBatch.share()`), each file
 * parsed, transformed, and converted exactly as the in-process loop does
 * (`packages/orchestration/src/execution/jssg-batch.ts`). Per file it posts
 * `{ outcome }` or `{ error }` ("JSSG failed for '<path>': ..."); a module
 * that does not load fails the whole chunk.
 *
 * Each worker evaluates an artifact once per run, so module-level state is
 * per worker, not per run.
 */
import { parentPort } from "node:worker_threads";
import { loadBinding, runTransform } from "./index.js";
import { importArtifact } from "./loader.js";

const native = loadBinding();
const modules = new Map();

const describe = (error) =>
  error instanceof Error ? `${error.name}: ${error.message}` : String(error);

async function transformChunk({ batchId, runId, artifact, start, paths, contents }) {
  const key = `${runId}\0${artifact.hash}`;
  let loaded = modules.get(key);
  if (loaded === undefined) {
    loaded = importArtifact(artifact, runId);
    modules.set(key, loaded);
  }
  let transform;
  try {
    transform = await loaded;
    if (typeof transform !== "function") {
      throw new Error(`jssg codemod modules must export a function. ${typeof transform} found.`);
    }
  } catch (error) {
    return { fatal: `JSSG failed for '${paths[0]}': ${describe(error)}` };
  }
  const batch = native.JssgBatch.attach(batchId);
  const { targetRoot, params } = batch;
  const options = batch.options();
  const roots = await batch.parse(start, contents);
  const results = [];
  for (const [offset, root] of roots.entries()) {
    const path = paths[offset];
    if (root === null) {
      results.push({ outcome: { path, edits: [] } });
      continue;
    }
    let ran;
    try {
      ran = await runTransform(targetRoot, params, () =>
        transform(root, { ...options, params: { ...options.params } }),
      );
    } catch (error) {
      results.push({ error: `JSSG failed for '${path}': ${describe(error)}` });
      continue;
    }
    try {
      results.push({
        outcome: batch.finish(start + offset, root, ran.result, ran.staged) ?? { path, edits: [] },
      });
    } catch (error) {
      results.push({ error: error instanceof Error ? error.message : String(error) });
    }
  }
  return { results };
}

parentPort.on("message", ({ job, ...chunk }) => {
  transformChunk(chunk).then(
    (reply) => parentPort.postMessage({ job, ...reply }),
    (error) => parentPort.postMessage({ job, fatal: describe(error) }),
  );
});

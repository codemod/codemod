/**
 * A pool of worker threads that run JSSG transforms (`./worker.js`), so a
 * batch uses every core instead of one JS thread. The main thread plans the
 * batch and builds any workspace index; workers attach to it by id
 * (`JssgBatch.share()`), parse their chunk, and run its transforms.
 *
 * Workers start on first use, keep the process alive only while they have
 * work, and are reused across batches and runs. Each receives an artifact's
 * source once.
 */
import { availableParallelism } from "node:os";
import { Worker } from "node:worker_threads";

/**
 * Workers per process: `CODEMOD_JSSG_WORKERS`, else the core count capped at
 * 4. Each worker holds its own isolate and parsed trees (about 200 to 300 MB
 * on a heavy workspace codemod), so 4 keeps the peak near one serial run's.
 */
export function poolSize(env = process.env) {
  const configured = Number.parseInt(env.CODEMOD_JSSG_WORKERS ?? "", 10);
  if (Number.isInteger(configured) && configured >= 1) return configured;
  return Math.min(availableParallelism(), 4);
}

export function createTransformPool(size = poolSize()) {
  const workers = [];
  const queue = [];
  const jobs = new Map();
  let nextJob = 0;

  const spawn = () => {
    const worker = new Worker(new URL("./worker.js", import.meta.url), {
      env: process.env,
      stdout: false,
      stderr: false,
    });
    const slot = { worker, busy: false, artifacts: new Set() };
    worker.unref();
    worker.on("message", ({ job, results, fatal }) => {
      const pending = jobs.get(job);
      jobs.delete(job);
      slot.busy = false;
      worker.unref();
      if (fatal !== undefined) pending?.reject(new Error(fatal));
      else pending?.resolve(results);
      pump();
    });
    const lost = (error) => {
      const index = workers.indexOf(slot);
      if (index === -1) return;
      workers.splice(index, 1);
      for (const [job, pending] of jobs) {
        if (pending.slot === slot) {
          jobs.delete(job);
          pending.reject(error);
        }
      }
      pump();
    };
    worker.on("error", lost);
    worker.on("exit", (code) => lost(new Error(`JSSG transform worker exited with code ${code}`)));
    workers.push(slot);
    return slot;
  };

  const pump = () => {
    while (queue.length > 0) {
      let slot = workers.find((candidate) => !candidate.busy);
      if (slot === undefined && workers.length < size) slot = spawn();
      if (slot === undefined) return;
      const { job, chunk } = queue.shift();
      const pending = jobs.get(job);
      pending.slot = slot;
      slot.busy = true;
      slot.worker.ref();
      const key = `${chunk.runId}\0${chunk.artifact.hash}`;
      const artifact = slot.artifacts.has(key)
        ? { name: chunk.artifact.name, hash: chunk.artifact.hash }
        : chunk.artifact;
      slot.artifacts.add(key);
      slot.worker.postMessage({ job, ...chunk, artifact });
    }
  };

  return {
    size,
    /**
     * Run one chunk: `{ batchId, runId, artifact, start, paths, contents }`.
     * Resolves with one `{ outcome } | { error }` per file.
     */
    run(chunk) {
      const job = nextJob++;
      return new Promise((resolve, reject) => {
        jobs.set(job, { resolve, reject });
        queue.push({ job, chunk });
        pump();
      });
    },
    /** Stop every worker. */
    async close() {
      const closing = workers.splice(0);
      await Promise.all(closing.map(({ worker }) => worker.terminate()));
    },
  };
}

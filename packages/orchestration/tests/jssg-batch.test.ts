/**
 * The portable batch loop (`createJssgBatchDispatch`) against a scripted
 * engine: what a failing file does under each `onFileError` policy, and that
 * a pool engine (`transformRange`) answers exactly as the serial loop. The
 * real engines run in the runtimes' e2e tests.
 */
import { describe, expect, it } from "vitest";
import {
  createJssgBatchDispatch,
  type JssgBatch,
  type JssgBatchEngine,
  type RangeResult,
} from "../src/execution/jssg-batch.ts";
import { PROTOCOL_VERSION, type JssgOperation } from "../src/index.ts";
import { ref } from "./helpers.ts";

/** Upper-cases each file; a file whose content is `boom` throws. */
const transform = (root: unknown) => {
  if (root === "boom") throw new Error("boom");
  return String(root).toUpperCase();
};

function engine(pool: boolean): JssgBatchEngine<string, string> {
  return {
    open(_config, paths) {
      const finish = (index: number, ran: string) => ({
        path: paths[index]!,
        edits: [{ path: paths[index]!, content: ran }],
      });
      const batch: JssgBatch<string, string> = {
        module: "t.jssg.js",
        indexesWorkspace: false,
        index: async () => {},
        parse: async (_start, contents) => contents,
        call: async (_index, root, fn) => fn(root, {}) as string,
        finish: (index, _root, ran) => finish(index, ran),
        close: () => {},
      };
      if (!pool) return batch;
      return {
        ...batch,
        call: () => Promise.reject(new Error("the pool engine ran a file on this thread")),
        parallelism: 2,
        transformRange: async (start, contents) =>
          contents.map((content, offset): RangeResult => {
            try {
              return { outcome: finish(start + offset, transform(content)) };
            } catch {
              return { error: `JSSG failed for '${paths[start + offset]}': Error: boom` };
            }
          }),
      };
    },
    load: async () => transform,
    describeError: (error) => `${(error as Error).name}: ${(error as Error).message}`,
  };
}

const files = ["a", "boom", "c"].map((content, index) => ({ path: `${index}.ts`, content }));

function run(options: { pool?: boolean; onFileError?: JssgOperation["onFileError"] } = {}) {
  const operation: JssgOperation = {
    kind: "jssg",
    transform: ref("t"),
    language: "typescript",
    ...(options.onFileError === undefined ? {} : { onFileError: options.onFileError }),
  };
  return createJssgBatchDispatch({ engine: engine(options.pool ?? false) })({
    protocolVersion: PROTOCOL_VERSION,
    commandId: "t",
    operation,
    context: { targetRoot: "/repo", files, artifact: { source: "" } },
  });
}

const skipped = {
  files: [
    { path: "0.ts", edits: [{ path: "0.ts", content: "A" }] },
    { path: "1.ts", edits: [] },
    { path: "2.ts", edits: [{ path: "2.ts", content: "C" }] },
  ],
  failures: [{ path: "1.ts", error: "JSSG failed for '1.ts': Error: boom" }],
};

describe("createJssgBatchDispatch", () => {
  it("skips a file whose transform throws, reports it, and keeps the other files", async () => {
    expect(await run()).toMatchObject({ status: "succeeded", output: skipped });
  });

  it("fails the command on a throwing file when onFileError is 'fail'", async () => {
    expect(await run({ onFileError: "fail" })).toMatchObject({
      status: "failed",
      error: { message: "JSSG failed for '1.ts': Error: boom" },
    });
  });

  it("a pool engine answers with the same outcomes and failures as the serial loop", async () => {
    expect(await run({ pool: true })).toMatchObject({ status: "succeeded", output: skipped });
  });
});

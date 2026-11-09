/**
 * The portable batch loop (`createJssgBatchDispatch`) against a scripted
 * engine: a pool engine (`transformRange`) answers exactly as the serial
 * loop. What a failing file does under each `onFileError` policy is in the
 * cargo `jssg_parity` specs, on the real engine.
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

function run(pool: boolean) {
  const operation: JssgOperation = { kind: "jssg", transform: ref("t"), language: "typescript" };
  return createJssgBatchDispatch({ engine: engine(pool) })({
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
  it("a pool engine answers with the same outcomes and failures as the serial loop", async () => {
    expect(await run(true)).toMatchObject({ status: "succeeded", output: skipped });
  });
});

import { deepStrictEqual, rejects } from "node:assert";
import test, { describe } from "node:test";

import { SandboxedModule } from "../js/module.js";
import { NodeSandbox } from "../js/node.js";

async function run(
  code: string,
  inputs: Record<string, unknown> = {},
): Promise<Record<string, unknown>> {
  const module = new SandboxedModule(new NodeSandbox(), {}, { test: code });
  return module.invoke("test", inputs);
}

describe("runtime basics", () => {
  test("can run a simple module", async () => {
    deepStrictEqual(
      await run(
        `export default function() {
        return { result: "HELLO" }
      }`,
      ),
      { result: "HELLO" },
    );
  });

  test("can accept arguments", async () => {
    deepStrictEqual(
      await run(
        `export default function({test}) {
        return { result: test }
      }`,
        { test: "HELLO" },
      ),
      { result: "HELLO" },
    );
  });

  test("supports async export", async () => {
    deepStrictEqual(
      await run(
        `export default async function({test}) {
        return new Promise((resolve) => resolve({ result: test }));
      }`,
        { test: "HELLO" },
      ),
      { result: "HELLO" },
    );
  });
});

describe("runtime errors", () => {
  test("handles invalid module", async () => {
    await rejects(run("export"), /invalid export syntax/);

    await rejects(run("FOO"), /Error converting from js 'undefined' into type 'function'/);
  });

  test("handles errors thrown", async () => {
    await rejects(
      run(
        `export default function() {
        throw new Error("OH NOES");
      }`,
      ),
      /OH NOES/,
    );
  });

  test("handles errors thrown in async functions", async () => {
    await rejects(
      run(
        `export default async function() {
        throw new Error("OH NOES");
      }`,
      ),
      /OH NOES/,
    );
  });

  test("handles syntax errors", async () => {
    await rejects(
      run(
        `export default async function() {
        foo += 1;
      }`,
      ),
      /'foo' is not defined/,
    );
  });
});

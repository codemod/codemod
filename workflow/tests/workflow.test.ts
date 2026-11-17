import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { runWorkflow } from "codemod/testing";

const workflow = new URL("../src/workflow.ts", import.meta.url);

describe("codebase roaster workflow", () => {
  test("an empty catch clause is reported as reliability/empty-catch", async () => {
    const run = await runWorkflow(workflow, {
      target: { "src/a.ts": "try { run(); } catch (error) {}\n" },
    });
    assert.equal(run.output?.findings["reliability/empty-catch"], 1);
  });

  test("a file with no hotspots gives a clean run", async () => {
    const run = await runWorkflow(workflow, { target: { "src/a.ts": "export const one = 1;\n" } });
    assert.deepEqual(run.output?.findings, {});
  });

  test("files under node_modules are not analyzed", async () => {
    const run = await runWorkflow(workflow, {
      target: { "node_modules/pkg/a.js": "try { run(); } catch (error) {}\n" },
    });
    assert.deepEqual(run.output?.findings, {});
  });

  test("Rust unwrap density is reported as rust/unwrap-expect-density", async () => {
    const run = await runWorkflow(workflow, {
      target: { "src/main.rs": "fn main() { a().unwrap(); b().unwrap(); c().expect(\"c\"); }\n" },
    });
    assert.equal(run.output?.findings["rust/unwrap-expect-density"], 1);
  });

  test("each slice family runs as its own step", async () => {
    const run = await runWorkflow(workflow, { target: { "src/a.ts": "" } });
    assert.deepEqual(
      run.steps.map((step) => step.label),
      [
        "Analyze complexity",
        "Analyze coupling",
        "Analyze reliability",
        "Analyze rust",
        "Analyze sensitive",
        "Analyze state",
        "Analyze structure",
        "Analyze testing",
        "Analyze types",
      ],
    );
  });
});

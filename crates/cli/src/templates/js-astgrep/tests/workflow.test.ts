import { describe, test } from "node:test";
import { assertTree, fixtures, runWorkflow } from "codemod/testing";

const workflow = new URL("../src/workflow.ts", import.meta.url);

describe("{name}", () => {
  // Each directory under tests/fixtures is one case: its `input` is copied
  // into a temporary directory, the workflow runs there, and the files it
  // leaves must match `expected`. Run with CODEMOD_UPDATE_FIXTURES=1 to
  // update the expected files from the runs.
  for (const fixture of fixtures(new URL("./fixtures", import.meta.url))) {
    test(fixture.name, async () => {
      const run = await runWorkflow(workflow, {
        target: fixture.input,
        params: fixture.params,
      });
      assertTree(run, fixture.expected);
    });
  }
});

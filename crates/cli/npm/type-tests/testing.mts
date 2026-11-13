// Type tests for `codemod/testing`: this file only needs to type-check.
import { assertTree, fixtures, runWorkflow, type WorkflowRun } from "../testing/index.mjs";

// A `URL`, as `new URL("./tests", import.meta.url)` gives (this check has no DOM or Node types).
declare const testsUrl: { readonly href: string; readonly protocol: string };

export async function examples(): Promise<void> {
  for (const fixture of fixtures(testsUrl)) {
    const run: WorkflowRun = await runWorkflow("workflow.ts", {
      target: fixture.input,
      params: fixture.params,
    });
    assertTree(run, fixture.expected, { ignoreWhitespace: true });
  }

  const run = await runWorkflow(testsUrl, {
    target: { "src/a.ts": "import moment from 'moment';\n" },
    until: "Rewrite imports",
    given: { "Find moment imports": { outputs: [{ path: "src/a.ts" }] } },
    capabilities: ["fs"],
    approve: false,
  });
  const status: "succeeded" | "failed" | "canceled" | "awaiting-approval" | "stopped" = run.status;
  const deleted: string | null | undefined = run.changes["src/a.ts"];
  const given: boolean = run.step("Find moment imports").given;
  assertTree(run.files, { "src/a.ts": "" });
  void [status, deleted, given, run.diff(), run.logs[0]?.step];

  const workflowModule = { default: async (_context: { params: Record<string, unknown> }) => ({}) };
  await runWorkflow(workflowModule, { path: "workflow.ts", target: {}, host: "node" });
  await runWorkflow("workflow.ts", { target: {}, host: "quickjs" });
  // @ts-expect-error hosts are named
  await runWorkflow("workflow.ts", { target: {}, host: "deno" });

  // @ts-expect-error target is required
  await runWorkflow("workflow.ts", {});
  // @ts-expect-error capabilities are named
  await runWorkflow("workflow.ts", { target: {}, capabilities: ["network"] });
  // @ts-expect-error given values are JSON
  await runWorkflow("workflow.ts", { target: {}, given: { Step: new Map() } });
  // @ts-expect-error files are text
  assertTree({ "a.ts": 1 }, {});
}

// Type tests for `codemod/testing`: this file only needs to type-check.
import {
  assertTree,
  fixtures,
  runCallback,
  runWorkflow,
  type CallbackRun,
  type JsonValue,
  type WorkflowRun,
} from "../testing/index.mjs";

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
  const status:
    | "succeeded"
    | "failed"
    | "canceled"
    | "paused"
    | "rejected"
    | "awaiting-approval"
    | "stopped" = run.status;
  const deleted: string | null | undefined = run.changes["src/a.ts"];
  const given: boolean = run.step("Find moment imports").given;
  assertTree(run.files, { "src/a.ts": "" });
  const exitCode: number | null = run.exitCode;
  void [status, deleted, given, exitCode, run.diff(), run.logs[0]?.step];

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

export async function callbackExamples(): Promise<void> {
  for (const fixture of fixtures(testsUrl)) {
    const run: CallbackRun = await runCallback("src/migrate.ts", {
      target: fixture.input,
      language: "typescript",
      semanticAnalysis: "workspace",
      capabilities: ["fs"],
    });
    assertTree(run, fixture.expected);
    const first: JsonValue | undefined = run.outputs[0]?.data;
    const failed: string | undefined = run.failures[0]?.error;
    void [first, failed, run.diff()];
  }
  // @ts-expect-error language is required
  await runCallback("src/migrate.ts", { target: {} });
  // @ts-expect-error semantic analysis modes are named
  await runCallback("src/migrate.ts", { target: {}, language: "tsx", semanticAnalysis: "project" });
  // @ts-expect-error a callback has no params
  await runCallback("src/migrate.ts", { target: {}, language: "tsx", params: {} });
}

// Type tests for `codemod/host` and `codemod/register`: this file only needs to type-check.
import { runWorkflow, type HostRunReport } from "../runtime/host.mjs";
import { resolveCodemodModule } from "../register.mjs";

export async function examples(): Promise<void> {
  const report: HostRunReport = await runWorkflow("./my-codemod", {
    target: "./repo",
    params: { to: "dayjs" },
    store: "local",
    dryRun: true,
  });
  const status: "succeeded" | "failed" | "canceled" | "awaiting-approval" | "stopped" =
    report.status;
  const modified: number = report.stats.filesModified;
  void [status, modified, report.steps[0]?.label, report.events[0]?.event];

  await runWorkflow({ default: () => null }, { path: "workflow.ts" });
  await runWorkflow("./my-codemod", { resume: report.runId, store: "local" });

  const url: string | undefined = resolveCodemodModule("codemod:workflow");
  void url;

  // @ts-expect-error stores are named
  await runWorkflow("./my-codemod", { store: "sqlite" });
}

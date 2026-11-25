// The report every repo-maintenance workflow returns, and what the local
// dashboard (`dash run`) collects from `run_finished.output`.
//
// The workflow only says what it checked. The collector adds repo, commit,
// branch, time, duration, CLI version and exit code.

import { fail } from "codemod/workflow";

export type Status = "pass" | "fail" | "warn";

export type Finding = {
  message: string;
  file?: string;
  line?: number;
  url?: string;
};

export type Check = {
  id: string;
  title: string;
  status: Status;
  metrics?: Readonly<Record<string, number>>;
  findings?: readonly Finding[];
};

export type Report = {
  workflow: string;
  checks: readonly Check[];
};

/**
 * Returns the report, and fails the run when a check failed.
 *
 * Failing = `fail()` from `codemod/workflow`: the run ends `failed` and
 * `workflow run` exits 2, and the report stays the run's output
 * (`run_finished.output` with `--format jsonl`, `workflow status --format json`,
 * `runWorkflow(...).output`). The message below is `run_finished.error`.
 */
export function finish(report: Report): Report {
  const failed = report.checks.filter((check) => check.status === "fail");
  if (failed.length) {
    const names = failed.map((check) => `${check.id} (${check.findings?.length ?? 0} findings)`);
    fail(`${report.workflow}: ${failed.length} check(s) failed: ${names.join(", ")}`, {
      output: report,
    });
  }
  return report;
}

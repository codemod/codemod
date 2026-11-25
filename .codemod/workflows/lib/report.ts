// The report every repo-maintenance workflow returns, and what the local
// dashboard (`dash run`) collects. Userland: no CLI API involved.
//
// The workflow only says what it checked. The collector adds repo, commit,
// branch, time, duration, CLI version and exit code.

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
 * Prefix of the one line `finish` prints with the report as JSON.
 *
 * Why a printed line: `--format jsonl` has no event with the workflow's return
 * value (or step results), and a run that fails has `output: null` in the run
 * store, so the line (a `run_log` event) is the only place a failing run's
 * report survives.
 */
export const REPORT_MARKER = "::codemod-report::";

/** Finds the report in a run's log lines (JSONL `run_log` lines, or `runWorkflow`'s `logs`). */
export function readReport(lines: readonly string[]): Report | undefined {
  const line = lines.findLast((l) => l.startsWith(REPORT_MARKER));
  return line ? (JSON.parse(line.slice(REPORT_MARKER.length)) as Report) : undefined;
}

/**
 * Prints the report, then fails the run when a check failed.
 *
 * Failing = throwing from the workflow body: the run ends `failed` and
 * `workflow run` exits 1, with the message below as `run_finished.error`.
 * (A failed step that the body catches leaves the run `succeeded`, exit 0;
 * exit 2 only comes from files failing in a `forEachFile` step.)
 */
export function finish(report: Report): Report {
  console.log(REPORT_MARKER + JSON.stringify(report));
  const failed = report.checks.filter((check) => check.status === "fail");
  if (failed.length) {
    const names = failed.map((check) => `${check.id} (${check.findings?.length ?? 0} findings)`);
    throw new Error(`${report.workflow}: ${failed.length} check(s) failed: ${names.join(", ")}`);
  }
  return report;
}

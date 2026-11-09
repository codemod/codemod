/**
 * `@codemod.com/orchestration/dashboard`: the run model of the local run
 * dashboard (`RunMonitor`: run events projected into the snapshot and event
 * stream `ui.html` renders). The server that hosted it behind `--dashboard`
 * ran only on the removed `node` runtime; it comes back in the Rust CLI, fed
 * by the QuickJS host's run events.
 */
export { RunMonitor, projectEvent } from "./monitor.ts";
export type {
  CommandState,
  CommandView,
  DashboardEnvelope,
  DashboardEvent,
  DashboardSnapshot,
  MonitorOptions,
  RunStatus,
  SchedulerView,
} from "./monitor.ts";

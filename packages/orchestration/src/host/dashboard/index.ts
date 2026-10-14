/** `@codemod.com/orchestration/dashboard`: the local run dashboard the CLI hosts behind `--dashboard`. */
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
export {
  DEFAULT_RUN_LIMIT,
  DashboardSession,
  RunConflictError,
  RunNotFoundError,
} from "./session.ts";
export type {
  ConflictCode,
  ControlResult,
  RestartResult,
  RunOutcome,
  RunSummary,
  SessionNotice,
  SessionOptions,
} from "./session.ts";
export {
  handleApi,
  matchRoute,
  openEventStream,
  parseStreamId,
  runsView,
  sseFrame,
  streamId,
} from "./api.ts";
export type {
  ApiRequestContext,
  ApiResponse,
  ApiRoute,
  MatchedRoute,
  Method,
  RouteName,
  RunFrame,
  RunsView,
} from "./api.ts";
export { DASHBOARD_HOST, startDashboard } from "./server.ts";
export type { Dashboard, DashboardOptions } from "./server.ts";

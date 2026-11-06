/**
 * `@codemod.com/orchestration` on Node: the portable API (`portable.ts`),
 * with the Node host installed, plus the Node-only pieces: the build step,
 * the bridge runtime, agents and assessment, and the runtime registry.
 */
import "./host/node-host.ts";

export * from "./portable.ts";
export { BuildError, buildFile, buildModule, loadWorkflow } from "./bundle/build.ts";
export { spawnBridge, executeJssg } from "./execution/bridge.ts";
export type { BridgeProcessOptions, JssgExecutionOptions } from "./execution/bridge.ts";
export {
  DEFAULT_ASSESSMENT_FILE_CONCURRENCY,
  DEFAULT_ASSESSMENT_TIMEOUT_MS,
  MAX_ASSESSMENT_RETRIES,
  executeAssessment,
  executeFileAssessment,
} from "./execution/assessment.ts";
export type {
  AssessmentClient,
  AssessmentClientFactory,
  AssessmentExecutorOptions,
} from "./execution/assessment.ts";
export {
  AGENT_ENV_ALLOWLIST,
  AGENT_ENV_PREFIXES,
  BRIDGE_SECRETS_ENV,
  EXTERNAL_ENV_ALLOWED,
  agentEnvironment,
  agentLaunch,
  externalAgentEnvProblem,
  isSecretEnvName,
} from "./execution/agent-env.ts";
export { BridgeExecutor, DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS } from "./execution/executor.ts";
export {
  EXCHANGE_DIR_ENV,
  exchangeRootCandidates,
  readResponse,
  resolveExchangeRoot,
} from "./execution/exchange.ts";
export type { BridgeOptions } from "./execution/executor.ts";
export { NODE_HOST, createNodeHost, nodeHost } from "./host/node-host.ts";
export {
  RuntimeRegistry,
  RuntimeUnavailableError,
  UnknownRuntimeError,
  assertAvailable,
  defaultRuntimes,
  loadExecutable,
  prepareLaunch,
  runWorkflow,
} from "./host/runtimes/registry.ts";
export type {
  LoadedExecutable,
  PreparedLaunch,
  RunWorkflowOptions,
} from "./host/runtimes/registry.ts";
export { bridgeRuntime, defaultBridgePath } from "./host/runtimes/bridge.ts";
export { notAvailableYet } from "./host/runtimes/types.ts";
export type {
  ExecutorContext,
  LaunchRequest,
  RuntimeAvailability,
  RuntimeProvider,
  RuntimeSettings,
} from "./host/runtimes/types.ts";

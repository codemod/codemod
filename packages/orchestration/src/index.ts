/**
 * `@codemod.com/orchestration` on Node: the portable API (`portable.ts`),
 * with the Node host installed, plus the Node-only pieces (the build step,
 * the runtime registry and providers) and the agent and assessment
 * executors.
 */
import "./host/node-host.ts";

export * from "./portable.ts";
export { BuildError, buildFile, buildModule, loadWorkflow } from "./bundle/build.ts";
export {
  DEFAULT_ASSESSMENT_FILE_CONCURRENCY,
  DEFAULT_ASSESSMENT_TIMEOUT_MS,
  MAX_ASSESSMENT_RETRIES,
  executeAssessment,
  executeAssessmentOperation,
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
  AGENT_SECRETS_ENV,
  EXTERNAL_ENV_ALLOWED,
  agentEnvironment,
  agentLaunch,
  externalAgentEnvProblem,
  isSecretEnvName,
} from "./execution/agent-env.ts";
export {
  AGENT_BINARY,
  AGENT_BIN_ENV,
  DEFAULT_EXTERNAL_AGENT_TIMEOUT_MS,
  executeAgent,
} from "./execution/agent.ts";
export type { AgentExecutionOptions } from "./execution/agent.ts";
export { defaultAgentPath } from "./host/runtimes/agent-bin.ts";
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
export { nodeRuntime } from "./host/runtimes/node.ts";
export { NodeExecutor } from "./host/runtimes/node-executor.ts";
export type { NodeExecutorOptions } from "./host/runtimes/node-executor.ts";
export { createNapiJssgEngine } from "./host/runtimes/node-jssg.ts";
export { notAvailableYet } from "./host/runtimes/types.ts";
export type {
  ExecutorContext,
  LaunchRequest,
  RuntimeAvailability,
  RuntimeProvider,
  RuntimeSettings,
} from "./host/runtimes/types.ts";

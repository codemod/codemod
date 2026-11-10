/**
 * `@codemod.com/orchestration/portable`: everything that runs without Node.
 * Authoring, static composition, `run()` with replay/history/scheduling, the
 * `Host` seam, and the portable JSSG + shell execution path. Nothing reachable from this entry imports `node:*` or
 * touches `process` (`tests/portable.test.ts`), so a QuickJS workflow bundle
 * can alias `@codemod.com/orchestration` to it. A host must be installed
 * (`setHost` or `globalThis[HOST_GLOBAL]`) before the first run.
 *
 * The Node entry (`src/index.ts`) re-exports all of this, installs the Node
 * host, and adds the Node-only pieces: the build step, the runtime registry
 * and providers, and the agent and assessment executors.
 */
export {
  agent,
  assessment,
  isRunnable,
  jssg,
  parseAgentJson,
  shell,
} from "./authoring/runnable.ts";
export type {
  AgentBackendOptions,
  AgentOptions,
  AgentOutput,
  AgentRunnable,
  AssessmentAskContext,
  AssessmentOptions,
  AssessmentOutput,
  AssessmentRunnable,
  InputOf,
  JssgOptions,
  JssgRunnable,
  OperationFor,
  OperationKind,
  OutputOf,
  Runnable,
  ShellOutput,
  ShellRunnable,
} from "./authoring/runnable.ts";
export type {
  JssgLanguages,
  JssgSelector,
  JssgTransform,
  JssgTransformResult,
  JssgTypes,
} from "./authoring/transform.ts";
export type { ArtifactStore, JssgArtifact } from "./core/artifacts.ts";
export { isCommand } from "./authoring/command.ts";
export type { Command, Invocation, JssgInvocation } from "./authoring/command.ts";
export { NoActiveRunError } from "./authoring/context.ts";
export { normalizeTarget } from "./authoring/target.ts";
export {
  executableIr,
  executableRequiresInput,
  isExecutable,
  isParallel,
  isSequence,
  parallel,
  sequence,
} from "./authoring/composition.ts";
export type {
  CompositionIr,
  CompositionIrNode,
  Executable,
  ExecutableOutput,
  OperationIr,
  Parallel,
  ParallelIr,
  Sequence,
  SequenceIr,
  Stage,
  StageInput,
  StageOutput,
  DynamicIr,
} from "./authoring/composition.ts";
export { dynamic } from "./authoring/dynamic.ts";
export type { Awaitable, Dynamic } from "./authoring/dynamic.ts";
export { run } from "./runtime/run.ts";
export type { RootInput, RunOptions, RunResult, RunSettings } from "./runtime/run.ts";
export { guard, SchemaError } from "./authoring/schema.ts";
export type { InferOutput, StandardSchemaV1 } from "./authoring/schema.ts";
export {
  NUMERIC_TOLERANCE,
  RESERVED_KEYS,
  assessmentResultProblem,
  getAssessmentAsk,
  isAssessmentQuestions,
  questionsProblem,
  setAssessmentAsk,
} from "./core/assessment.ts";
export type {
  AnswerFor,
  AssessmentAnswer,
  AssessmentAskFn,
  AssessmentEntry,
  AssessmentFile,
  AssessmentFileResult,
  AssessmentQuestion,
  AssessmentQuestions,
  AssessmentResult,
  AssessmentState,
  AssessmentUsage,
  ChoiceAnswer,
  ChoiceQuestion,
  RefusedAnswer,
  ScoreAnswer,
  ScoreQuestion,
  YesNoAnswer,
  YesNoQuestion,
} from "./core/assessment.ts";
export { DECISION_PROVIDERS } from "./core/decision.ts";
export type { DecisionProvider } from "./core/decision.ts";
export { canonicalJson } from "./core/json.ts";
export type { Json } from "./core/json.ts";
export {
  AGENT_BACKENDS,
  BUILTIN_AGENT_TOOLS,
  CLAUDE_CODE_TOOLS,
  CODEX_SANDBOXES,
  DEFAULT_BUILTIN_AGENT_TOOLS,
  DEFAULT_CLAUDE_CODE_TOOLS,
  DEFAULT_CODEX_SANDBOX,
  PROTOCOL_VERSION,
  agentBackendProblem,
  isAgentBackend,
  isArtifactRef,
  isFileOutcomes,
  isJson,
  isOperation,
  isOperationCompletion,
  isOperationRequest,
  isSelector,
  isTarget,
  parseCompletion,
} from "./core/protocol.ts";
export type {
  AgentBackend,
  AgentBackendKind,
  AgentOperation,
  BuiltinAgentTool,
  ClaudeCodeTool,
  CodexSandbox,
  AssessmentOperation,
  ArtifactRef,
  BatchFile,
  CompletionError,
  CompletionStatus,
  Edit,
  ShellOperation,
  FileOutcome,
  JssgFileErrorPolicy,
  JssgOperation,
  Operation,
  OperationCompletion,
  OperationRequest,
  RequestContext,
  Selector,
  SemanticAnalysis,
  Target,
} from "./core/protocol.ts";
export { PathEscapeError, isSafeRelativePath, resolveInsideRoot } from "./core/paths.ts";
export {
  HOST_GLOBAL,
  HostNotInstalledError,
  errorCode,
  findHost,
  getHost,
  installDefaultHost,
  setHost,
} from "./core/host.ts";
export type {
  AsyncContextVariable,
  Host,
  HostChild,
  HostDirEntry,
  HostFileStat,
  HostFs,
  HostOs,
  HostPath,
  HostSpawnOptions,
  HostSpawnResult,
  HostTimer,
} from "./core/host.ts";
export { createPosixPath } from "./core/posix.ts";
export {
  comparePaths,
  discoverGlobalExcludesPath,
  languageExtensions,
  selectFiles,
} from "./execution/files.ts";
export type { Selection } from "./execution/files.ts";
export { executeJssgCommand } from "./execution/jssg.ts";
export type { JssgCommandOptions, JssgDispatch } from "./execution/jssg.ts";
export { createJssgBatchDispatch, describeError } from "./execution/jssg-batch.ts";
export type {
  JssgBatch,
  JssgBatchConfig,
  JssgBatchDispatchOptions,
  JssgBatchEngine,
  TransformArtifact,
  TransformFunction,
} from "./execution/jssg-batch.ts";
export { executeShell, normalizeShellOutput } from "./execution/shell.ts";
export type { ShellExecutionOptions } from "./execution/shell.ts";
export type { OperationExecutor } from "./execution/operation-executor.ts";
export {
  MemoryHistoryStore,
  emptyHistory,
  scheduledCommands,
  completions,
  finalOutput,
} from "./core/history.ts";
export type { History, HistoryEvent, HistoryStore, ScheduledCommand } from "./core/history.ts";
export {
  AdmissionScheduler,
  CAPACITY_ENV,
  DEFAULT_WEIGHTS,
  SchedulingExecutor,
  defaultCapacity,
  installedHostFacts,
  weightOf,
} from "./execution/scheduler.ts";
export type {
  OperationWeights,
  Permit,
  SchedulerHost,
  SchedulerOptions,
  SchedulerStats,
} from "./execution/scheduler.ts";
export { ReplayGate } from "./runtime/gate.ts";
export type { CommandGate } from "./runtime/gate.ts";
export { CollectingSink, nullSink } from "./core/events.ts";
export type { EventSink, RunEvent } from "./core/events.ts";
export {
  DuplicateCommandIdError,
  InvocationError,
  NondeterminismError,
  OperationError,
  CompositionValidationError,
  TargetValidationError,
} from "./core/errors.ts";
export type { NondeterminismKind } from "./core/errors.ts";

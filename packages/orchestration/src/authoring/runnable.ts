/**
 * Runnable descriptors: pure, serializable-ish descriptions of an operation.
 * They know how to turn typed input into a wire `Operation` and how to turn
 * a completion output back into typed data. They never execute anything.
 *
 * Every descriptor is callable. Calling it creates a `Command` (see
 * `command.ts`): `inspect()`, `lint({ id })`, `migrate({ input, target, id })`.
 * Only JSSG accepts `target`.
 */
import {
  createCommand,
  type BoundInvocation,
  type Command,
  type FlowInvocation,
} from "./command.ts";
import {
  assessmentResultProblem,
  isAssessmentEntry,
  questionsProblem,
  type AssessmentQuestions,
  type AssessmentResult,
  type AssessmentState,
} from "../core/assessment.ts";
import { cloneJson, type Json } from "../core/json.ts";
import { isSafeRelativePath } from "../core/paths.ts";
import {
  DEFAULT_BUILTIN_AGENT_TOOLS,
  DEFAULT_CLAUDE_CODE_TOOLS,
  DEFAULT_CODEX_SANDBOX,
  agentBackendProblem,
  isArtifactRef,
  isSelector,
  type AgentBackend,
  type AgentOperation,
  type BuiltinAgentTool,
  type ClaudeCodeTool,
  type CodexSandbox,
  type ArtifactRef,
  type AssessmentOperation,
  type JssgOperation,
  type Operation,
  type Selector,
  type SemanticAnalysis,
  type Target,
} from "../core/protocol.ts";
import { validate, type StandardSchemaV1 } from "./schema.ts";
import type { JssgSelector, JssgTransform, JssgTypes } from "./transform.ts";

export type OperationKind = Operation["kind"];

/** The operation variant whose `kind` matches `K`. */
export type OperationFor<K extends OperationKind> = Extract<Operation, { kind: K }>;

export interface Runnable<I = void, O = unknown, K extends OperationKind = OperationKind> {
  readonly kind: K;
  readonly name: string;
  readonly input?: StandardSchemaV1<unknown, I>;
  readonly output?: StandardSchemaV1<unknown, O>;
  /** Build the wire operation from validated input. Must be pure. */
  toOperation(input: I): OperationFor<K>;
  /**
   * Turn a succeeded completion's output into typed data. Must be pure. The
   * `operation` is the wire operation `toOperation` built for this invocation;
   * runnables whose decoding depends on resolved runtime data (e.g.
   * assessment questions) read it from the operation instead of closing over
   * static definitions.
   */
  decode(output: Json | undefined, operation: OperationFor<K>): Promise<O>;
}

export type InputOf<R> = R extends Runnable<infer I, unknown> ? I : never;
export type OutputOf<R> = R extends Runnable<unknown, infer O> ? O : never;

/** A runnable's data fields without its call signature. */
type Descriptor<R> = { [P in keyof R]: R[P] };

export function isRunnable(value: unknown): value is Runnable<unknown, unknown> {
  if (typeof value !== "function") return false;
  const candidate = value as unknown as Runnable;
  return typeof candidate.kind === "string" && typeof candidate.toOperation === "function";
}

interface ShellOptions<I, O> {
  name: string;
  /** Shell command, or a pure function of the validated input. */
  command: string | ((input: I) => string);
  env?: Record<string, string> | ((input: I) => Record<string, string>);
  input?: StandardSchemaV1<unknown, I>;
  /**
   * When present, stdout is parsed as JSON and validated. Without it the
   * output is `{ stdout: string }`.
   */
  output?: StandardSchemaV1<unknown, O>;
}

export interface ShellOutput {
  stdout: string;
}

export interface ShellRunnable<I = void, O = ShellOutput> extends Runnable<I, O, "shell"> {
  (): Command<O, I>;
  (options: FlowInvocation): Command<O, I>;
  (options: BoundInvocation<I>): Command<O>;
}

export function shell<I = void, O = ShellOutput>(options: ShellOptions<I, O>): ShellRunnable<I, O> {
  return callable<I, O, "shell", ShellRunnable<I, O>>({
    kind: "shell",
    name: options.name,
    input: options.input,
    output: options.output,
    toOperation(input) {
      const command =
        typeof options.command === "function" ? options.command(input) : options.command;
      const env = typeof options.env === "function" ? options.env(input) : options.env;
      return env && Object.keys(env).length > 0
        ? { kind: "shell", command, env }
        : { kind: "shell", command };
    },
    async decode(output, _operation) {
      const stdout = readField(output, "stdout", "shell");
      if (!options.output) return { stdout } as O;
      let parsed: unknown;
      try {
        parsed = JSON.parse(stdout);
      } catch (error) {
        throw new Error(`shell '${options.name}' stdout is not JSON: ${(error as Error).message}`);
      }
      return validate(options.output, parsed, `shell '${options.name}' output`);
    },
  });
}

function readField(output: Json | undefined, field: string, kind: string): string {
  if (
    output &&
    typeof output === "object" &&
    !Array.isArray(output) &&
    typeof output[field] === "string"
  ) {
    return output[field];
  }
  throw new Error(`${kind} completion did not contain string ${field}`);
}

interface DataOptions<I, O> {
  name: string;
  input?: StandardSchemaV1<unknown, I>;
  output?: StandardSchemaV1<unknown, O>;
}

/**
 * A JSSG definition: the transform written inline next to the workflow.
 * `language` types `root` and `options` for the transform (and the selector)
 * exactly as the language modules of `codemod:ast-grep` do.
 */
export interface JssgOptions<L extends string, I, O> extends DataOptions<I, O> {
  language: L;
  include?: string[];
  exclude?: string[];
  semanticAnalysis?: SemanticAnalysis;
  /**
   * Optional static prefilter: files without a match never reach the
   * sandbox. Plain ast-grep rule data; unlike a legacy `getSelector()` it is
   * never executed and never fills `options.matches`.
   */
  selector?: JssgSelector<JssgTypes<L>>;
  /**
   * The one transform, with the documented `(root, options)` contract. The
   * build step (`bundle/build.ts`) replaces it with an `ArtifactRef` before the
   * module runs; a function reaching this call means the module was not
   * built, which is refused.
   */
  transform: JssgTransform<JssgTypes<L>, I, O> | ArtifactRef;
}

/**
 * JSSG codemod. The only runnable whose invocation may carry a `target`. The
 * target is command content (it travels on the wire and replay compares it),
 * not command identity.
 *
 * `transform` is the artifact identity the build assigned (`{ name, hash }`,
 * no path), so the recorded command is the same on every checkout; the
 * executor supplies the source from its artifact store.
 */
export interface JssgRunnable<I = void, O = unknown> extends Runnable<I, O, "jssg"> {
  readonly transform: ArtifactRef;
  readonly selector?: Selector;
  toOperation(input: I, target?: Target): JssgOperation;
  (): Command<O, I>;
  (options: FlowInvocation & { target?: Target }): Command<O, I>;
  (options: BoundInvocation<I> & { target?: Target }): Command<O>;
}

export function jssg<L extends string, I = void, O = unknown>(
  options: JssgOptions<L, I, O>,
): JssgRunnable<I, O> {
  const { transform, selector } = assertJssgOptions(options);
  return callable<I, O, "jssg", JssgRunnable<I, O>>({
    kind: "jssg",
    transform,
    selector,
    name: options.name,
    input: options.input,
    output: options.output,
    toOperation(input, target) {
      const operation: JssgOperation = {
        kind: "jssg",
        transform: { name: transform.name, hash: transform.hash },
        language: options.language,
      };
      if (options.include !== undefined) operation.include = options.include;
      if (options.exclude !== undefined) operation.exclude = options.exclude;
      if (options.semanticAnalysis !== undefined) {
        operation.semanticAnalysis = options.semanticAnalysis;
      }
      if (selector !== undefined) operation.selector = selector;
      if (target !== undefined) operation.target = target;
      if (input !== undefined) operation.input = input as Json;
      return operation;
    },
    decode: (output, _operation) =>
      validate(options.output, output, `jssg '${options.name}' output`),
  });
}

function assertJssgOptions(options: {
  name: string;
  language: string;
  include?: string[];
  exclude?: string[];
  semanticAnalysis?: SemanticAnalysis;
  selector?: unknown;
  transform: unknown;
}): { transform: ArtifactRef; selector: Selector | undefined } {
  const where = `jssg '${options.name}'`;
  if (typeof options.transform === "function") {
    throw new Error(
      `${where}: inline transform was not extracted; run the workflow with codemod-workflow or load it with loadWorkflow() so the build step can bundle it`,
    );
  }
  if (!isArtifactRef(options.transform)) {
    throw new Error(`${where}: transform must be the { name, hash } reference the build produced`);
  }
  if (options.language.trim() === "") throw new Error("jssg language must not be empty");
  for (const [name, patterns] of [
    ["include", options.include],
    ["exclude", options.exclude],
  ] as const) {
    if (patterns?.length === 0 || patterns?.some((pattern) => pattern.trim() === "")) {
      throw new Error(`jssg ${name} must contain non-empty glob patterns`);
    }
  }
  const semantic = options.semanticAnalysis;
  if (typeof semantic === "object" && semantic.root !== undefined) {
    if (semantic.mode !== "workspace") {
      throw new Error("jssg semanticAnalysis.root requires workspace mode");
    }
    if (!isSafeRelativePath(semantic.root)) {
      throw new Error("jssg semanticAnalysis.root must be a safe relative path");
    }
  }
  if (options.selector !== undefined && !isSelector(options.selector)) {
    throw new Error(
      `${where}: selector must be JSON with a non-empty 'rule' and optional 'constraints' and 'utils'`,
    );
  }
  return { transform: options.transform, selector: options.selector as Selector | undefined };
}

export interface AgentOutput {
  /** The agent's final response. */
  text: string;
}

/**
 * Which agent loop runs the step. Each variant takes only the settings its
 * backend can enforce; omitted settings take that backend's safe default.
 *
 * - `builtin` (the default backend): `codemod-ai`. `tools` defaults to
 *   `DEFAULT_BUILTIN_AGENT_TOOLS` (no `bash`); `maxSteps` bounds its turns.
 * - `claude-code`: the installed `claude` CLI. `tools` defaults to
 *   `DEFAULT_CLAUDE_CODE_TOOLS` (no `Bash`). No turn limit is enforceable.
 * - `codex`: the installed `codex` CLI. `sandbox` defaults to
 *   `workspace-write`. Its tools are Codex's own and cannot be listed.
 */
export type AgentBackendOptions =
  | { kind: "builtin"; tools?: readonly BuiltinAgentTool[]; maxSteps?: number }
  | { kind: "claude-code"; tools?: readonly ClaudeCodeTool[] }
  | { kind: "codex"; sandbox?: CodexSandbox };

export interface AgentOptions<I, O> extends DataOptions<I, O> {
  /** The task. Validated input, when declared, is appended to it by the executor. */
  prompt: string;
  /** Default `{ kind: "builtin" }`. Recorded in the command, so changing it replays as `changed`. */
  backend?: AgentBackendOptions;
}

const AGENT_OPTION_KEYS = ["name", "prompt", "input", "output", "backend"];

/** Fill a backend's defaults; the result is the exact operation `backend`. */
function resolveAgentBackend(options: AgentBackendOptions | undefined): unknown {
  const backend = (options === undefined ? { kind: "builtin" } : options) as Record<
    string,
    unknown
  >;
  if (typeof backend !== "object" || backend === null) return backend;
  switch (backend.kind) {
    case "builtin":
      return {
        ...backend,
        tools: [
          ...((backend.tools as BuiltinAgentTool[] | undefined) ?? DEFAULT_BUILTIN_AGENT_TOOLS),
        ],
      };
    case "claude-code":
      return {
        ...backend,
        tools: [...((backend.tools as ClaudeCodeTool[] | undefined) ?? DEFAULT_CLAUDE_CODE_TOOLS)],
      };
    case "codex":
      return { ...backend, sandbox: backend.sandbox ?? DEFAULT_CODEX_SANDBOX };
    default:
      return backend;
  }
}

export interface AgentRunnable<I = void, O = AgentOutput> extends Runnable<I, O, "agent"> {
  (): Command<O, I>;
  (options: FlowInvocation): Command<O, I>;
  (options: BoundInvocation<I>): Command<O>;
}

/**
 * Agent step: the selected backend works on the repository and may change
 * files. Without `output` the result is
 * `{ text }`, the final response. With `output` the operation carries
 * `responseFormat: "json"` (the bridge tells the agent to answer with JSON
 * only) and the text, bare or inside one markdown JSON fence, is parsed and
 * validated.
 */
export function agent<I = void, O = AgentOutput>(options: AgentOptions<I, O>): AgentRunnable<I, O> {
  const where = `agent '${options.name}'`;
  const unknown = Object.keys(options).find((key) => !AGENT_OPTION_KEYS.includes(key));
  if (unknown !== undefined) {
    throw new Error(
      `${where}: unknown option '${unknown}'; backend settings such as tools belong in 'backend'`,
    );
  }
  const resolved = resolveAgentBackend(options.backend);
  const problem = agentBackendProblem(resolved);
  if (problem !== undefined) throw new Error(`${where}: ${problem}`);
  const backend = cloneJson(resolved) as AgentBackend;
  return callable<I, O, "agent", AgentRunnable<I, O>>({
    kind: "agent",
    name: options.name,
    input: options.input,
    output: options.output,
    toOperation(input) {
      const operation: AgentOperation = {
        kind: "agent",
        prompt: options.prompt,
        backend: cloneJson(backend),
      };
      if (input !== undefined) operation.input = input as Json;
      if (options.output !== undefined) operation.responseFormat = "json";
      return operation;
    },
    async decode(output, _operation) {
      const text = readField(output, "text", "agent");
      if (!options.output) return { text } as O;
      const parsed = parseAgentJson(text);
      if ("error" in parsed) {
        throw new Error(
          `${where} output: ${parsed.error}; the response starts ${JSON.stringify(excerpt(text))}`,
        );
      }
      return validate(options.output, parsed.value, `${where} output`);
    },
  });
}

const FENCE = /```[ \t]*(?:json)?[ \t]*\r?\n([\s\S]*?)\r?\n?[ \t]*```/giu;

/**
 * An agent's JSON answer: the whole text, or the content of its only
 * ```json (or bare ```) fence. Several fences are ambiguous and refused.
 */
export function parseAgentJson(text: string): { value: unknown } | { error: string } {
  try {
    return { value: JSON.parse(text) };
  } catch {
    // Not bare JSON; look for a fence.
  }
  const fences = [...text.matchAll(FENCE)];
  if (fences.length === 0) return { error: "response is neither JSON nor one ```json block" };
  if (fences.length > 1) {
    return { error: `response has ${fences.length} fenced blocks; expected exactly one` };
  }
  try {
    return { value: JSON.parse(fences[0]![1]!) };
  } catch (error) {
    return { error: `the fenced block is not JSON (${(error as Error).message})` };
  }
}

function excerpt(text: string, limit = 200): string {
  return text.length > limit ? `${text.slice(0, limit)}…` : text;
}

/**
 * What `ask` returns: the explicit model state and the assessment questions,
 * resolved together from validated runtime input so that dynamic criteria
 * cannot diverge from state derivation.
 */
export interface AskResult<Q extends AssessmentQuestions> {
  /** Everything the model may see. Nothing else is sent. */
  state: AssessmentState;
  /** Named choice, score, and noul questions; answers come back under the same ids. */
  questions: Q;
}

export interface AssessmentOptions<I, Q extends AssessmentQuestions> {
  name: string;
  input?: StandardSchemaV1<unknown, I>;
  /**
   * Resolve the explicit model state and assessment questions from validated
   * input. Both are returned together so that dynamic criteria (choice options
   * derived from input, score levels computed at run time) stay in lockstep
   * with the state the model evaluates. The resolved questions drive operation
   * serialization, output validation, history, and replay.
   */
  ask: (input: I) => AskResult<Q>;
  /**
   * Pin a model or alias (e.g. `jev-1.13.0`). Omitted, the executor's default
   * answers: `TYPESAFE_DEFAULT_MODEL`, else `jev-latest`.
   */
  model?: string;
}

export interface AssessmentRunnable<
  I = void,
  Q extends AssessmentQuestions = AssessmentQuestions,
> extends Runnable<I, AssessmentResult<Q>, "assessment"> {
  /** Resolve state and questions from input; exposed for tests and inspection. */
  readonly ask: (input: I) => AskResult<Q>;
  (): Command<AssessmentResult<Q>, I>;
  (options: FlowInvocation): Command<AssessmentResult<Q>, I>;
  (options: BoundInvocation<I>): Command<AssessmentResult<Q>>;
}

/**
 * Read-only System One assessment: explicit state, typed questions, no
 * tools. The result carries every answer's probabilities and confidence, the
 * model that answered, and token usage; it decides nothing. Routing on those
 * numbers is workflow code.
 *
 * The `ask` function resolves both state and questions from validated input,
 * so dynamic criteria (choice options derived from input, score levels
 * computed at run time) cannot diverge from the state the model evaluates.
 * Questions are validated when the operation is built, not at definition
 * time, because they may depend on runtime input. The concrete resolved
 * questions are serialized in the operation, validated in the decoder, and
 * recorded in history for replay.
 */
export function assessment<I = void, Q extends AssessmentQuestions = AssessmentQuestions>(
  options: AssessmentOptions<I, Q>,
): AssessmentRunnable<I, Q> {
  const where = `assessment '${options.name}'`;
  if (options.name.trim() === "") throw new Error("assessment name must not be empty");
  if (typeof options.ask !== "function") {
    throw new Error(`${where}: ask must be a function`);
  }
  if (options.model !== undefined && options.model.trim() === "") {
    throw new Error(`${where}: model must not be empty`);
  }
  return callable<I, AssessmentResult<Q>, "assessment", AssessmentRunnable<I, Q>>({
    kind: "assessment",
    name: options.name,
    input: options.input,
    ask: options.ask,
    toOperation(input) {
      const result = options.ask(input);
      if (
        typeof result !== "object" ||
        result === null ||
        Array.isArray(result) ||
        !("state" in result) ||
        !("questions" in result)
      ) {
        throw new Error(`${where}: ask must return { state, questions }`);
      }
      if (!isAssessmentEntry(result.state)) {
        throw new Error(`${where}: state must be non-empty text, a JSON object, or a JSON array`);
      }
      const qProblem = questionsProblem(result.questions);
      if (qProblem !== undefined) throw new Error(`${where}: ${qProblem}`);
      const operation: AssessmentOperation = {
        kind: "assessment",
        state: result.state,
        questions: cloneJson(result.questions),
      };
      if (options.model !== undefined) operation.model = options.model;
      return operation;
    },
    async decode(output, operation) {
      const invalid = assessmentResultProblem(operation.questions, output);
      if (invalid !== undefined) throw new Error(`${where} output is invalid: ${invalid}`);
      return output as unknown as AssessmentResult<Q>;
    },
  });
}

/**
 * Attach a descriptor's fields to the function that creates its commands.
 * Function `name`/`length` are non-writable, so `Object.assign` cannot be used.
 */
function callable<I, O, K extends OperationKind, R extends Runnable<I, O, K>>(
  descriptor: Descriptor<R>,
): R {
  const invoke = (options?: unknown) =>
    createCommand(invoke as unknown as Runnable<unknown, O>, options);
  for (const [key, value] of Object.entries(descriptor)) {
    if (value === undefined) continue;
    Object.defineProperty(invoke, key, { value, enumerable: true, configurable: true });
  }
  return invoke as unknown as R;
}

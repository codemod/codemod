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
  questionsProblem,
  setAssessmentAsk,
  type AssessmentAskFn,
  type AssessmentFileResult,
  type AssessmentQuestions,
  type AssessmentResult,
} from "../core/assessment.ts";
import type { AssessmentFile } from "../core/assessment.ts";
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
 * What `ask` receives per file: the selected file's path and content, plus
 * the validated workflow input (or `undefined` when the assessment has no
 * input schema). `ask` returns QUESTIONS ONLY — the model state (file
 * content, path, and input) is assembled automatically.
 */
export interface AssessmentAskContext<I = void> {
  file: AssessmentFile;
  input: I;
}

export interface AssessmentOptions<I, Q extends AssessmentQuestions> {
  name: string;
  /** File selection globs relative to the working directory. At least one required. */
  include: string[];
  /** File exclusion globs relative to the working directory. */
  exclude?: string[];
  input?: StandardSchemaV1<unknown, I>;
  /**
   * Define the assessment questions for one file. Receives the file (path and
   * content) and the validated workflow input so that questions and choice
   * options can be dynamic per file. Returns questions only — the model state
   * is assembled automatically from `{ file: { path, content }, input? }`.
   *
   * The resolved questions drive per-file SDK calls and output validation.
   * They are validated when the executor runs each file.
   */
  ask: (context: AssessmentAskContext<I>) => Q;
  /**
   * Pin a model or alias (e.g. `jev-1.13.0`). Omitted, the executor's default
   * answers: `TYPESAFE_DEFAULT_MODEL`, else `jev-latest`.
   */
  model?: string;
}

/**
 * The per-file assessment result: an ordered array with explicit file
 * attribution, preserving deterministic selector order regardless of
 * completion order.
 */
export type AssessmentOutput<Q extends AssessmentQuestions> = AssessmentFileResult<Q>[];

/**
 * A file-oriented assessment is a first-class `Runnable`: callable (creates
 * a `Command`), composable in `sequence()`/`parallel()`, directly usable as
 * root, and its single command replays from history with zero file I/O.
 */
export interface AssessmentRunnable<
  I = void,
  Q extends AssessmentQuestions = AssessmentQuestions,
> extends Runnable<I, AssessmentOutput<Q>, "assessment"> {
  readonly include: readonly string[];
  readonly exclude: readonly string[] | undefined;
  /** Resolve questions for a given file; exposed for tests and inspection. */
  readonly ask: (context: AssessmentAskContext<I>) => Q;
  (): Command<AssessmentOutput<Q>, I>;
  (options: FlowInvocation): Command<AssessmentOutput<Q>, I>;
  (options: BoundInvocation<I>): Command<AssessmentOutput<Q>>;
}

/**
 * Sentinel questions for the batch operation. The real per-file questions are
 * resolved by the executor at execution time (via the non-enumerable `__ask`
 * on the operation) and embedded in the completion output alongside each
 * file's result. The sentinel must pass `questionsProblem` so the operation
 * is a valid `AssessmentOperation` on the wire.
 */
const BATCH_SENTINEL: AssessmentQuestions = {
  __batch: { type: "noul", instructions: "file-oriented batch assessment" },
};

/**
 * File-oriented read-only System One assessment. Selects files using
 * JSSG-style include/exclude globs, reads each file's content, and runs one
 * TypeSafe assessment per file. File selection, reading, and SDK calls happen
 * entirely in the execution layer (using the `--target` root), never in the
 * authoring layer.
 *
 * Assessment is a proper `Runnable`: it can be a workflow root, compose in
 * `sequence()`/`parallel()`, and its single command replays from history with
 * zero file I/O or SDK calls.
 *
 * Each file's model state is automatically assembled as
 * `{ file: { path, content }, input? }`. The `ask` function receives the
 * file and input and defines QUESTIONS ONLY — it may produce dynamic
 * criteria per file.
 *
 * The result is an ordered array of `{ file, assessment }` entries
 * preserving the deterministic selector order regardless of completion
 * order. Each file's response is validated against the exact questions
 * resolved for that file.
 *
 * Assessment is read-only and tool-free. The include/exclude globs are the
 * explicit privacy boundary: only matched file contents are sent to the
 * model. Repository-level assessment is intentionally not supported; to
 * assess a summary, have a preceding shell or JSSG step materialize a
 * single summary file and assess that file.
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
  if (!Array.isArray(options.include) || options.include.length === 0) {
    throw new Error(`${where}: include must be a non-empty list of glob patterns`);
  }
  if (options.include.some((pattern) => typeof pattern !== "string" || pattern.trim() === "")) {
    throw new Error(`${where}: include patterns must be non-empty strings`);
  }
  if (options.exclude !== undefined) {
    if (!Array.isArray(options.exclude) || options.exclude.length === 0) {
      throw new Error(`${where}: exclude must be a non-empty list of glob patterns`);
    }
    if (options.exclude.some((pattern) => typeof pattern !== "string" || pattern.trim() === "")) {
      throw new Error(`${where}: exclude patterns must be non-empty strings`);
    }
  }
  const include = [...options.include];
  const exclude = options.exclude ? [...options.exclude] : undefined;

  return callable<I, AssessmentOutput<Q>, "assessment", AssessmentRunnable<I, Q>>({
    kind: "assessment",
    name: options.name,
    include,
    exclude,
    ask: options.ask,
    input: options.input,
    toOperation(input: I): AssessmentOperation {
      // State encodes the batch identity: include/exclude and optional input.
      // This is what canonicalJson compares for replay.
      const state: Record<string, Json> = {
        include: cloneJson(include) as Json,
      };
      if (exclude !== undefined) state.exclude = cloneJson(exclude) as Json;
      if (input !== undefined) state.input = cloneJson(input) as Json;

      const operation: AssessmentOperation = {
        kind: "assessment",
        state,
        questions: cloneJson(BATCH_SENTINEL),
      };
      if (options.model !== undefined) operation.model = options.model;

      // Attach the per-file question resolver via the module-scoped WeakMap in
      // core/assessment.ts. The WeakMap is invisible to every serialization path
      // (JSON.stringify, canonicalJson, cloneJson, history) and cannot collide
      // with wire fields. The execution layer reads it back via getAssessmentAsk
      // to distinguish live batch operations from replayed/single-file ones.
      setAssessmentAsk(operation, options.ask as AssessmentAskFn);

      return operation;
    },
    async decode(output: Json | undefined, _operation: AssessmentOperation) {
      // Empty result: no files matched.
      if (output === undefined || output === null) return [] as AssessmentOutput<Q>;
      if (!Array.isArray(output)) {
        throw new Error(`${where}: expected an array of per-file results`);
      }
      if (output.length === 0) return [] as AssessmentOutput<Q>;
      const results: AssessmentFileResult<Q>[] = [];
      for (const entry of output) {
        if (typeof entry !== "object" || entry === null || Array.isArray(entry)) {
          throw new Error(`${where}: each result must be an object`);
        }
        const record = entry as Record<string, unknown>;
        if (typeof record.file !== "string") {
          throw new Error(`${where}: each result must have a string 'file' field`);
        }
        const qProblem = questionsProblem(record.questions);
        if (qProblem !== undefined) {
          throw new Error(`${where} output for '${record.file}': invalid questions: ${qProblem}`);
        }
        const rProblem = assessmentResultProblem(
          record.questions as AssessmentQuestions,
          record.assessment,
        );
        if (rProblem !== undefined) {
          throw new Error(`${where} output for '${record.file}': ${rProblem}`);
        }
        results.push({
          file: record.file,
          assessment: record.assessment as unknown as AssessmentResult<Q>,
        });
      }
      return results;
    },
  } as Descriptor<AssessmentRunnable<I, Q>>);
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

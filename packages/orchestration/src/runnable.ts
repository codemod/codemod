/**
 * Runnable descriptors: pure, serializable-ish descriptions of an operation.
 * They know how to turn typed input into a wire `Operation` and how to turn
 * a completion output back into typed data. They never execute anything.
 *
 * Every descriptor is callable. Calling it creates a `Command` (see
 * `command.ts`): `inspect()`, `lint({ id })`, `migrate({ input, target, id })`.
 * Only JSSG accepts `target`.
 */
import { createCommand, type Command, type InvokeArgs, type JssgInvokeArgs } from "./command.ts";
import type { Json } from "./json.ts";
import { isSafeRelativePath } from "./paths.ts";
import {
  isArtifactRef,
  isSelector,
  type ArtifactRef,
  type JssgOperation,
  type Operation,
  type Selector,
  type SemanticAnalysis,
  type Target,
} from "./protocol.ts";
import { validate, type StandardSchemaV1 } from "./schema.ts";
import type { JssgSelector, JssgTransform, JssgTypes } from "./transform.ts";

export type OperationKind = Operation["kind"];

export interface Runnable<I = void, O = unknown, K extends OperationKind = OperationKind> {
  readonly kind: K;
  readonly name: string;
  readonly input?: StandardSchemaV1<unknown, I>;
  readonly output?: StandardSchemaV1<unknown, O>;
  /** Build the wire operation from validated input. Must be pure. */
  toOperation(input: I): Operation;
  /** Turn a succeeded completion's output into typed data. Must be pure. */
  decode(output: Json | undefined): Promise<O>;
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

interface ExecOptions<I, O> {
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

export interface ExecOutput {
  stdout: string;
}

export interface ExecRunnable<I = void, O = ExecOutput> extends Runnable<I, O, "exec"> {
  (...args: InvokeArgs<I>): Command<O>;
}

export function exec<I = void, O = ExecOutput>(options: ExecOptions<I, O>): ExecRunnable<I, O> {
  return callable<I, O, "exec", ExecRunnable<I, O>>({
    kind: "exec",
    name: options.name,
    input: options.input,
    output: options.output,
    toOperation(input) {
      const command =
        typeof options.command === "function" ? options.command(input) : options.command;
      const env = typeof options.env === "function" ? options.env(input) : options.env;
      return env && Object.keys(env).length > 0
        ? { kind: "exec", command, env }
        : { kind: "exec", command };
    },
    async decode(output) {
      const stdout = readStdout(output);
      if (!options.output) return { stdout } as O;
      let parsed: unknown;
      try {
        parsed = JSON.parse(stdout);
      } catch (error) {
        throw new Error(`exec '${options.name}' stdout is not JSON: ${(error as Error).message}`);
      }
      return validate(options.output, parsed, `exec '${options.name}' output`);
    },
  });
}

function readStdout(output: Json | undefined): string {
  if (
    output &&
    typeof output === "object" &&
    !Array.isArray(output) &&
    typeof output.stdout === "string"
  ) {
    return output.stdout;
  }
  throw new Error("exec completion did not contain string stdout");
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
   * build step (`build.ts`) replaces it with an `ArtifactRef` before the
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
  (...args: JssgInvokeArgs<I>): Command<O>;
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
    decode: (output) => validate(options.output, output, `jssg '${options.name}' output`),
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

export interface AiRunnable<I = void, O = unknown> extends Runnable<I, O, "ai"> {
  (...args: InvokeArgs<I>): Command<O>;
}

/** AI step. No executor adapter exists yet; see README. */
export function ai<I = void, O = unknown>(
  options: DataOptions<I, O> & { prompt: string },
): AiRunnable<I, O> {
  return callable<I, O, "ai", AiRunnable<I, O>>({
    kind: "ai",
    name: options.name,
    input: options.input,
    output: options.output,
    toOperation: (input) =>
      input === undefined
        ? { kind: "ai", prompt: options.prompt }
        : { kind: "ai", prompt: options.prompt, input: input as Json },
    decode: (output) => validate(options.output, output, `ai '${options.name}' output`),
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

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
import type { JssgOperation, Operation, SemanticAnalysis, Target } from "./protocol.ts";
import { validate, type StandardSchemaV1 } from "./schema.ts";

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
 * JSSG codemod package. The only runnable whose invocation may carry a
 * `target`. The target is command content (it travels on the wire and replay
 * compares it), not command identity. The bridge executes trusted local
 * scripts; the harness can script completions.
 *
 * `script` is a safe relative path (`scripts/migrate.ts`), resolved by the
 * executor against its script root: the workflow file's directory for the
 * local CLI, or `BridgeOptions.scriptRoot`. It is never absolute so the
 * recorded command identity does not depend on where a checkout lives.
 */
export interface JssgRunnable<I = void, O = unknown> extends Runnable<I, O, "jssg"> {
  readonly script: string;
  toOperation(input: I, target?: Target): JssgOperation;
  (...args: JssgInvokeArgs<I>): Command<O>;
}

export function jssg<I = void, O = unknown>(
  options: DataOptions<I, O> & {
    script: string;
    language: string;
    include?: string[];
    exclude?: string[];
    semanticAnalysis?: SemanticAnalysis;
  },
): JssgRunnable<I, O> {
  assertJssgOptions(options);
  return callable<I, O, "jssg", JssgRunnable<I, O>>({
    kind: "jssg",
    script: options.script,
    name: options.name,
    input: options.input,
    output: options.output,
    toOperation(input, target) {
      const operation: JssgOperation = {
        kind: "jssg",
        script: options.script,
        language: options.language,
      };
      if (options.include !== undefined) operation.include = options.include;
      if (options.exclude !== undefined) operation.exclude = options.exclude;
      if (options.semanticAnalysis !== undefined) {
        operation.semanticAnalysis = options.semanticAnalysis;
      }
      if (target !== undefined) operation.target = target;
      if (input !== undefined) operation.input = input as Json;
      return operation;
    },
    decode: (output) => validate(options.output, output, `jssg '${options.name}' output`),
  });
}

function assertJssgOptions(options: {
  script: string;
  language: string;
  include?: string[];
  exclude?: string[];
  semanticAnalysis?: SemanticAnalysis;
}): void {
  if (options.script.trim() === "") throw new Error("jssg script must not be empty");
  if (!isSafeRelativePath(options.script)) {
    throw new Error(
      `jssg script '${options.script}' must be a relative path without '..' segments; it is resolved against the executor's script root`,
    );
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
    Object.defineProperty(invoke, key, { value, enumerable: true, configurable: true });
  }
  return invoke as unknown as R;
}

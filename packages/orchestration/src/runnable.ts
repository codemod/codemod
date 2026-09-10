/**
 * Runnable descriptors: pure, serializable-ish descriptions of an operation.
 * They know how to turn typed input into a wire `Operation` and how to turn
 * a completion output back into typed data. They never execute anything.
 */
import { TargetValidationError } from "./errors.ts";
import type { Json } from "./json.ts";
import type { JssgOperation, Operation, Target } from "./protocol.ts";
import { validate, type StandardSchemaV1 } from "./schema.ts";
import { normalizeTarget } from "./target.ts";

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

export function exec<I = void, O = ExecOutput>(options: ExecOptions<I, O>): Runnable<I, O, "exec"> {
  return {
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
  };
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
 * A JSSG runnable: the definition itself, or the definition bound to one
 * invocation target. `target` is command content (it travels on the wire and
 * replay compares it), not command identity: a targeted runnable keeps the
 * definition's name and therefore its default command id.
 */
export interface JssgRunnable<I = void, O = unknown> extends Runnable<I, O, "jssg"> {
  readonly package: string;
  readonly target?: Target;
}

/** The only invocation data the prototype attaches by calling a JSSG definition. */
export interface JssgInvocation {
  /** Repository area for this invocation; intersected with the definition's applicability. */
  target: Target;
}

/**
 * A JSSG definition is callable: `renameApi({ target })` returns a targeted
 * `JssgRunnable` usable in `plan()`, `parallel()`, or `w.run()`. Calling does
 * not schedule anything; the proposed callable form where a call creates a
 * lazy command (and also takes `input` and `id`) is future work, so those two
 * still go to `w.run(runnable, { input, id })`.
 */
export interface JssgDefinition<I = void, O = unknown> extends JssgRunnable<I, O> {
  readonly target?: undefined;
  (invocation: JssgInvocation): JssgRunnable<I, O>;
}

/**
 * JSSG codemod package. No executor adapter exists yet; see README. Tests use
 * scripted completions.
 */
export function jssg<I = void, O = unknown>(
  options: DataOptions<I, O> & { package: string },
): JssgDefinition<I, O> {
  const definition = (invocation: unknown) =>
    jssgRunnable(options, bindTarget(options.name, invocation));
  return withProperties(definition, jssgRunnable(options, undefined)) as JssgDefinition<I, O>;
}

function jssgRunnable<I, O>(
  options: DataOptions<I, O> & { package: string },
  target: Target | undefined,
): JssgRunnable<I, O> {
  const base = { kind: "jssg", package: options.package, name: options.name } as const;
  return {
    ...base,
    ...(target === undefined ? {} : { target }),
    input: options.input,
    output: options.output,
    toOperation(input) {
      const operation: JssgOperation = { kind: "jssg", package: options.package };
      if (target !== undefined) operation.target = target;
      if (input !== undefined) operation.input = input as Json;
      return operation;
    },
    decode: (output) => validate(options.output, output, `jssg '${options.name}' output`),
  };
}

function bindTarget(name: string, invocation: unknown): Target {
  const where = `jssg '${name}'`;
  if (typeof invocation !== "object" || invocation === null || Array.isArray(invocation)) {
    throw new TargetValidationError(where, "invocation must be an object: { target }");
  }
  for (const key of Object.keys(invocation)) {
    if (key === "target") continue;
    const hint =
      key === "input" || key === "id"
        ? `; pass '${key}' to w.run(runnable, { ${key} }) in this prototype`
        : "";
    throw new TargetValidationError(where, `unknown invocation field '${key}'${hint}`);
  }
  return normalizeTarget((invocation as { target?: unknown }).target, where);
}

/** Function `name`/`length` are non-writable, so `Object.assign` cannot be used here. */
function withProperties<F, P extends object>(fn: F, props: P): F & P {
  for (const [key, value] of Object.entries(props)) {
    Object.defineProperty(fn, key, { value, enumerable: true, configurable: true });
  }
  return fn as F & P;
}

/** AI step. No executor adapter exists yet; see README. */
export function ai<I = void, O = unknown>(
  options: DataOptions<I, O> & { prompt: string },
): Runnable<I, O, "ai"> {
  return {
    kind: "ai",
    name: options.name,
    input: options.input,
    output: options.output,
    toOperation: (input) =>
      input === undefined
        ? { kind: "ai", prompt: options.prompt }
        : { kind: "ai", prompt: options.prompt, input: input as Json },
    decode: (output) => validate(options.output, output, `ai '${options.name}' output`),
  };
}

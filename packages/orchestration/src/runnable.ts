/**
 * Runnable descriptors: pure, serializable-ish descriptions of an operation.
 * They know how to turn typed input into a wire `Operation` and how to turn
 * a completion output back into typed data. They never execute anything.
 */
import type { Json } from "./json.ts";
import type { Operation } from "./protocol.ts";
import { validate, type StandardSchemaV1 } from "./schema.ts";

export type OperationKind = Operation["kind"];

export interface Runnable<I = void, O = unknown> {
  readonly kind: OperationKind;
  readonly name: string;
  /** Read-only runnables may be placed in a `parallel` group. */
  readonly readOnly: boolean;
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
  readOnly?: boolean;
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

export function exec<I = void, O = ExecOutput>(options: ExecOptions<I, O>): Runnable<I, O> {
  return {
    kind: "exec",
    name: options.name,
    readOnly: options.readOnly ?? false,
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
  return "";
}

interface DataOptions<I, O> {
  name: string;
  readOnly?: boolean;
  input?: StandardSchemaV1<unknown, I>;
  output?: StandardSchemaV1<unknown, O>;
}

/**
 * JSSG codemod package. No executor adapter exists yet; see README. Tests use
 * scripted completions.
 */
export function jssg<I = void, O = unknown>(
  options: DataOptions<I, O> & { package: string },
): Runnable<I, O> {
  return {
    kind: "jssg",
    name: options.name,
    readOnly: options.readOnly ?? false,
    input: options.input,
    output: options.output,
    toOperation: (input) =>
      input === undefined
        ? { kind: "jssg", package: options.package }
        : { kind: "jssg", package: options.package, input: input as Json },
    decode: (output) => validate(options.output, output, `jssg '${options.name}' output`),
  };
}

/** AI step. No executor adapter exists yet; see README. */
export function ai<I = void, O = unknown>(
  options: DataOptions<I, O> & { prompt: string },
): Runnable<I, O> {
  return {
    kind: "ai",
    name: options.name,
    readOnly: options.readOnly ?? false,
    input: options.input,
    output: options.output,
    toOperation: (input) =>
      input === undefined
        ? { kind: "ai", prompt: options.prompt }
        : { kind: "ai", prompt: options.prompt, input: input as Json },
    decode: (output) => validate(options.output, output, `ai '${options.name}' output`),
  };
}

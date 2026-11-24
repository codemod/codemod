// `codemod/assess`: ask a decision model typed questions about a file.
//
// The question builders are plain data and work anywhere. `assess()` asks
// through the host the step installed (`globalThis.__assessHost`, from
// crates/workflow/src/assess/mod.rs), so it works in `forEachFile`
// callbacks and function steps, where its answers are recorded with the
// step; elsewhere it rejects. The Node.js host builds its own `assess` with
// `makeAssess` (crates/cli/npm/src/runtime/assess.mts).

interface OptionSpec {
  name: string;
  description?: string;
}

type Options = readonly string[] | Readonly<Record<string, string | null | undefined>>;

function optionsOf(what: string, options: Options): OptionSpec[] {
  if (Array.isArray(options)) {
    return options.map((name) => {
      if (typeof name !== "string") throw new TypeError(`${what}: each option must be a string`);
      return { name };
    });
  }
  if (!options || typeof options !== "object") {
    throw new TypeError(
      `${what}: options must be a list of names, or an object of names to descriptions`,
    );
  }
  return Object.entries(options).map(([name, description]) =>
    typeof description === "string" ? { name, description } : { name },
  );
}

function text(what: string, value: unknown): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new TypeError(`${what}: the question must be non-empty text`);
  }
  return value;
}

/** A yes/no question; the answer is `true` when the probability of yes is at least `threshold` (0.5). */
export function yesNo(
  instructions: string,
  options: { yes?: string; no?: string; threshold?: number } = {},
) {
  return Object.freeze({
    type: "yesNo" as const,
    instructions: text("yesNo", instructions),
    ...options,
  });
}

/** One of several options: `choice(["a", "b"])`, or with the question and descriptions. */
export function choice(instructionsOrOptions: string | Options, maybeOptions?: Options) {
  const instructions =
    typeof instructionsOrOptions === "string" ? text("choice", instructionsOrOptions) : undefined;
  const options = typeof instructionsOrOptions === "string" ? maybeOptions : instructionsOrOptions;
  if (options === undefined) throw new TypeError("choice: give the options");
  return Object.freeze({
    type: "choice" as const,
    ...(instructions === undefined ? {} : { instructions }),
    options: optionsOf("choice", options),
  });
}

/** A whole number from `min` to `max` (2 to 10 of them); `levels` describe each. */
export function number(
  instructions: string,
  range: { min: number; max: number; levels?: readonly string[] },
) {
  if (!range || typeof range.min !== "number" || typeof range.max !== "number") {
    throw new TypeError("number: give the range, { min, max }");
  }
  return Object.freeze({
    type: "number" as const,
    instructions: text("number", instructions),
    min: range.min,
    max: range.max,
    ...(range.levels ? { levels: [...range.levels] } : {}),
  });
}

/** Which of the options apply: each is asked as a yes/no question. */
export function list(
  instructions: string,
  options: Options,
  settings: { threshold?: number } = {},
) {
  return Object.freeze({
    type: "list" as const,
    instructions: text("list", instructions),
    options: optionsOf("list", options),
    ...settings,
  });
}

export interface AssessOptions {
  questions: Record<string, unknown>;
  context?: {
    imports?: boolean | number;
    dependents?: boolean | number;
    extra?: readonly string[];
    limit?: number;
  };
  provider?: string;
  model?: string;
  allowMoreContext?: number;
  timeout?: number | string;
}

/** Asks the decision model: the request as JSON, the assessment as JSON. */
export type AskHost = (request: string) => Promise<string>;

const OUTSIDE =
  "assess() works inside a forEachFile callback or a function step, where its answers are recorded with the step";

function milliseconds(timeout: number | string): number {
  if (typeof timeout === "number") return timeout;
  const match = /^\s*(\d+(?:\.\d+)?)\s*(ms|s|m|h)?\s*$/.exec(timeout);
  if (!match) throw new TypeError(`assess(): timeout '${timeout}' is not a duration such as "30s"`);
  const unit = { ms: 1, s: 1000, m: 60_000, h: 3_600_000 }[match[2] ?? "ms"] ?? 1;
  return Math.round(Number(match[1]) * unit);
}

/** `assess` that asks through the host `host()` gives, or rejects when it gives none. */
export function makeAssess(host: () => AskHost | undefined) {
  return async function assess(
    file: string | { readonly path: string; readonly text?: string },
    options: AssessOptions,
  ): Promise<unknown> {
    const ask = host();
    if (!ask) throw new Error(OUTSIDE);
    if (!options || typeof options !== "object") {
      throw new TypeError("assess(file, { questions }): give the questions");
    }
    const path = typeof file === "string" ? file : file?.path;
    if (typeof path !== "string") {
      throw new TypeError("assess(file, ...): give a forEachFile file, or a path in the target");
    }
    const request = {
      path,
      ...(typeof file === "object" && typeof file.text === "string" ? { text: file.text } : {}),
      questions: options.questions,
      ...(options.context === undefined ? {} : { context: options.context }),
      ...(options.provider === undefined ? {} : { provider: options.provider }),
      ...(options.model === undefined ? {} : { model: options.model }),
      ...(options.allowMoreContext === undefined
        ? {}
        : { allowMoreContext: options.allowMoreContext }),
      ...(options.timeout === undefined ? {} : { timeoutMs: milliseconds(options.timeout) }),
    };
    const answer = JSON.parse(await ask(JSON.stringify(request))) as
      | { ok: true; value: unknown }
      | { ok: false; error: string };
    if (!answer.ok) throw new Error(answer.error);
    return answer.value;
  };
}

interface AssessHost {
  ask: AskHost;
}

/** In the engine's isolates: the step's host, when it may assess. */
export const assess = makeAssess(() => {
  const host = (globalThis as { __assessHost?: AssessHost }).__assessHost;
  return host ? (request) => host.ask(request) : undefined;
});

// `codemod/assess`: ask a decision model (TypeSafe's System One by default,
// or OpenAI's Decisions API) typed questions about a file, with the files it
// imports as context. Inside `forEachFile` callbacks and function steps.

declare module "codemod:assess" {
  /** Option names, or names to descriptions (`null` for none). */
  type OptionsOf<O extends string> = readonly O[] | { readonly [K in O]: string | null };

  export interface YesNoQuestion {
    readonly type: "yesNo";
    readonly instructions: string;
    readonly yes?: string;
    readonly no?: string;
    readonly threshold?: number;
  }

  export interface ChoiceQuestion<O extends string = string> {
    readonly type: "choice";
    readonly instructions?: string;
    readonly options: readonly { readonly name: O; readonly description?: string }[];
  }

  export interface NumberQuestion {
    readonly type: "number";
    readonly instructions: string;
    readonly min: number;
    readonly max: number;
    readonly levels?: readonly string[];
  }

  export interface ListQuestion<O extends string = string> {
    readonly type: "list";
    readonly instructions: string;
    readonly options: readonly { readonly name: O; readonly description?: string }[];
    readonly threshold?: number;
  }

  export type Question = YesNoQuestion | ChoiceQuestion | NumberQuestion | ListQuestion;

  /** Questions by id; answers come back under the same ids. */
  export type Questions = { readonly [id: string]: Question };

  /**
   * A yes/no question. The answer is `true` when the probability of yes is
   * at least `threshold` (default 0.5); `details` has the probability.
   * `yes` and `no` say what counts as each.
   */
  export function yesNo(
    instructions: string,
    options?: { yes?: string; no?: string; threshold?: number },
  ): YesNoQuestion;

  /**
   * One of several options (2 to 255): `choice(["a", "b"])`, or with the
   * question and a description of each, `choice("Which?", { a: "When ...", b: null })`.
   */
  export function choice<const O extends string>(options: OptionsOf<O>): ChoiceQuestion<O>;
  export function choice<const O extends string>(
    instructions: string,
    options: OptionsOf<O>,
  ): ChoiceQuestion<O>;

  /**
   * A whole number from `min` to `max` (2 to 10 of them), `levels` describing
   * each. The answer is the probability-weighted number, so it may fall
   * between two; `details.likeliest` is the likeliest one.
   */
  export function number(
    instructions: string,
    range: { min: number; max: number; levels?: readonly string[] },
  ): NumberQuestion;

  /**
   * Which of the options (1 to 50) apply. Each is asked as a yes/no
   * question; the answer lists those at or above `threshold` (default 0.5).
   */
  export function list<const O extends string>(
    instructions: string,
    options: OptionsOf<O>,
    settings?: { threshold?: number },
  ): ListQuestion<O>;

  /** The typed answer to a question. */
  export type Answer<Q> = Q extends YesNoQuestion
    ? boolean
    : Q extends ChoiceQuestion<infer O>
      ? O
      : Q extends NumberQuestion
        ? number
        : Q extends ListQuestion<infer O>
          ? O[]
          : never;

  /** What the model said about a question besides its answer. */
  export type Detail<Q> = Q extends YesNoQuestion
    ? { probability: number }
    : Q extends ChoiceQuestion<infer O>
      ? { probabilities: Record<O, number>; confidence: number }
      : Q extends NumberQuestion
        ? { probabilities: Record<string, number>; likeliest: number; confidence: number }
        : Q extends ListQuestion<infer O>
          ? { probabilities: Record<O, number> }
          : never;

  export interface Assessment<Q extends Questions> {
    answers: { -readonly [K in keyof Q]: Answer<Q[K]> };
    details: { -readonly [K in keyof Q]: Detail<Q[K]> };
    /** The file, then every file shown with it, relative to the target. */
    context: string[];
    /** The provider that answered: `"systemone"` or `"openai"`. */
    provider: string;
    /** The model that answered, as the provider names it (`jev-1.13.0`). */
    model: string;
    /** Requests made (more than one when the model asked for more files) and tokens. */
    usage: { calls: number; inputTokens: number; outputTokens: number };
  }

  export interface AssessContext {
    /**
     * The files the file imports, followed through re-exports to the files
     * that define what it imports: `true` (the default) for those, a number
     * for that many levels of imports, `false` for none. JavaScript and
     * TypeScript files only.
     */
    imports?: boolean | number;
    /** The files of the target that import it: `true` for up to 10, or how many. Default `false`. */
    dependents?: boolean | number;
    /** More files to show, relative to the target. */
    extra?: readonly string[];
    /** Characters of context shown besides the file (default 100000); files past it are named, not shown. */
    limit?: number;
  }

  export interface AssessOptions<Q extends Questions> {
    questions: Q;
    context?: AssessContext;
    /** Over the workflow's codemod.yaml, the user config and the environment. */
    provider?: "systemone" | "openai";
    model?: string;
    /**
     * Let the model ask for one more file, from the imports one level deeper
     * and the files that import this one, up to this many times (at most 5).
     * Each time is another request.
     */
    allowMoreContext?: number;
    /** How long one request may take: milliseconds, or `"30s"`. Default one minute. */
    timeout?: number | string;
  }

  /**
   * Ask the run's decision model `options.questions` about `file` (a
   * `forEachFile` file, or a path relative to the target). Works in
   * `forEachFile` callbacks and function steps, where the answers are
   * recorded with the step, so a resumed run does not ask again; elsewhere
   * it rejects.
   *
   * ```ts
   * const { answers } = await assess(file, {
   *   questions: {
   *     handlesErrors: yesNo("Does this file handle the errors its async calls can throw?"),
   *     smell: choice(["swallowed", "logged-only", "none"]),
   *   },
   * });
   * answers.handlesErrors; // boolean
   * answers.smell; // "swallowed" | "logged-only" | "none"
   * ```
   */
  export function assess<const Q extends Questions>(
    file: string | { readonly path: string; readonly text?: string },
    options: AssessOptions<Q>,
  ): Promise<Assessment<Q>>;
}

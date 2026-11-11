/**
 * The assessment contract: explicit state evaluated against named, typed
 * questions (yes/no, choice, score) by a decision model, returning
 * probabilities and confidence. The shapes are this package's own; a
 * provider adapter (`execution/decision/`) translates them to its API and
 * back, and the environment picks the provider and model (`decision.ts`).
 *
 * An assessment is read-only: it has no repository access and no tools, and it
 * decides nothing. Workflow code reads the probabilities and owns routing.
 */
import type { Json } from "./json.ts";

/** Text, a JSON object, or a JSON array: state, instructions, and criteria. */
export type AssessmentEntry = string | { [key: string]: Json } | Json[];

/** Everything the model may look at. Nothing else is sent. */
export type AssessmentState = AssessmentEntry;

/** Yes/no. The answer is the probability of yes. */
export interface YesNoQuestion {
  type: "yesno";
  instructions: AssessmentEntry;
  criteria?: { yes?: AssessmentEntry | null; no?: AssessmentEntry | null };
}

/** One option from a set; keys are the options, values optional rubric text. */
export interface ChoiceQuestion<Option extends string = string> {
  type: "choice";
  instructions: AssessmentEntry;
  criteria: { [K in Option]: AssessmentEntry | null };
}

/** Ordered levels (at least two), rated from index 0. */
export interface ScoreQuestion {
  type: "score";
  instructions: AssessmentEntry;
  criteria: readonly (AssessmentEntry | null)[];
}

export type AssessmentQuestion = YesNoQuestion | ChoiceQuestion | ScoreQuestion;

/** Question ids are chosen by the author; answers come back under the same ids. */
export type AssessmentQuestions = { [id: string]: AssessmentQuestion };

export interface YesNoAnswer {
  type: "yesno";
  /** Probability of yes, 0 to 1. Yes/no answers carry no separate confidence. */
  yes: number;
}

export interface ChoiceAnswer<Option extends string = string> {
  type: "choice";
  /** The highest-probability option. */
  choice: Option;
  probabilities: { [K in Option]: number };
  confidence: number;
}

export interface ScoreAnswer {
  type: "score";
  /** Probability-weighted level; may fall between levels. */
  score: number;
  /** Level index (as a string) to its criteria entry. */
  legend: { [level: string]: Json };
  probabilities: { [level: string]: number };
  confidence: number;
}

/** The model declined to answer this question. */
export interface RefusedAnswer {
  type: "refused";
}

export type AssessmentAnswer = YesNoAnswer | ChoiceAnswer | ScoreAnswer | RefusedAnswer;

export type AnswerFor<Q> =
  | RefusedAnswer
  | (Q extends { type: "choice"; criteria: infer C }
      ? ChoiceAnswer<Extract<keyof C, string>>
      : Q extends { type: "score" }
        ? ScoreAnswer
        : Q extends { type: "yesno" }
          ? YesNoAnswer
          : never);

/** Token counts, each present only when the API reported it. */
export interface AssessmentUsage {
  inputTokens?: number;
  outputTokens?: number;
}

/** A succeeded assessment completion's `output`. */
export interface AssessmentResult<Q extends AssessmentQuestions = AssessmentQuestions> {
  /** The provider adapter that asked (`DECISION_PROVIDER`). */
  provider: string;
  /** The model that answered, as the provider names it (e.g. `jev-1.13.0` for `jev-latest`). */
  model: string;
  /** Whether `confidence` and the probabilities are calibrated probabilities. */
  calibrated: boolean;
  answers: { [K in keyof Q]: AnswerFor<Q[K]> };
  usage: AssessmentUsage;
}

/** One selected file as seen by the assessment: its root-relative path and UTF-8 content. */
export interface AssessmentFile {
  /** Root-relative `/`-separated path, in the deterministic walker order. */
  path: string;
  /** Full UTF-8 content of the file at selection time. */
  content: string;
}

/**
 * One entry in the ordered assessment result: the file that was assessed and its
 * typed result. The array preserves the deterministic selector order regardless
 * of completion order.
 */
export interface AssessmentFileResult<Q extends AssessmentQuestions = AssessmentQuestions> {
  file: string;
  assessment: AssessmentResult<Q>;
}

// ---------------------------------------------------------------------------
// Runtime-only hook: batch operation → per-file question resolver
// ---------------------------------------------------------------------------

/**
 * The per-file question resolver the authoring layer attaches to a batch
 * operation and the execution layer reads back. Stored in a module-scoped
 * `WeakMap` — never as a property — so it cannot collide with wire fields,
 * is invisible to every serialization path (`JSON.stringify`, `canonicalJson`,
 * `cloneJson`, history), and is garbage-collected with the operation object.
 *
 * Operations deserialized from history have no entry; that is how the executor
 * distinguishes a live batch from a replayed or single-file operation. The
 * store lives in `core` so both the authoring layer (setter) and the execution
 * layer (getter) can reach it without violating the one-way dependency rule
 * (`authoring → core`, `execution → core`).
 */
export type AssessmentAskFn = (context: { file: AssessmentFile; input: unknown }) => unknown;

const assessmentAskStore = new WeakMap<object, AssessmentAskFn>();

/** Attach an ask resolver to a batch assessment operation (authoring layer). */
export function setAssessmentAsk(operation: object, ask: AssessmentAskFn): void {
  assessmentAskStore.set(operation, ask);
}

/** Retrieve the ask resolver, or `undefined` for single/replayed operations (execution layer). */
export function getAssessmentAsk(operation: object): AssessmentAskFn | undefined {
  return assessmentAskStore.get(operation);
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const onlyKeys = (value: Record<string, unknown>, allowed: readonly string[]) =>
  Object.keys(value).every((key) => allowed.includes(key));

function isJsonValue(value: unknown): value is Json {
  if (value === null || typeof value === "string" || typeof value === "boolean") return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.every(isJsonValue);
  return isRecord(value) && Object.values(value).every(isJsonValue);
}

export function isAssessmentEntry(value: unknown): value is AssessmentEntry {
  if (typeof value === "string") return value.trim() !== "";
  return (Array.isArray(value) || isRecord(value)) && isJsonValue(value);
}

const isDescription = (value: unknown) => value === null || isAssessmentEntry(value);

/**
 * Keys that name `Object.prototype` machinery. As question ids or choice
 * options they would be answered under keys that ordinary object handling can
 * turn into prototype writes, so they are refused outright.
 */
export const RESERVED_KEYS: readonly string[] = ["__proto__", "constructor", "prototype"];

/**
 * Slack for floating-point noise in reported numbers: a score of
 * `2.0000000000000004` on a three-level rubric or a probability of
 * `1.0000000000000002` is accepted as is. Anything further out is invalid.
 */
export const NUMERIC_TOLERANCE = 1e-6;

const QUESTION_TYPES = ["yesno", "choice", "score"];

/**
 * Why a question is malformed, or `undefined` when it is valid. The same rules
 * apply when a runnable is defined and when an operation is decoded.
 */
export function questionProblem(question: unknown): string | undefined {
  if (!isRecord(question)) return "must be an object";
  if (!isAssessmentEntry(question.instructions)) {
    return "instructions must be non-empty text, a JSON object, or a JSON array";
  }
  if (!QUESTION_TYPES.includes(question.type as string)) {
    return "type must be 'yesno', 'choice', or 'score'";
  }
  if (!onlyKeys(question, ["type", "instructions", "criteria"])) return "has unknown fields";
  const { type, criteria } = question;
  if (type === "yesno") {
    if (criteria === undefined) return undefined;
    if (!isRecord(criteria) || !onlyKeys(criteria, ["yes", "no"])) {
      return "yesno criteria may only describe 'yes' and 'no'";
    }
  } else if (type === "choice") {
    if (!isRecord(criteria)) return "choice criteria must map options to descriptions";
    const options = Object.keys(criteria);
    if (options.length < 2) return "choice criteria must define at least two options";
    if (options.some((option) => option.trim() === "")) return "choice options must be non-empty";
    const reserved = options.find((option) => RESERVED_KEYS.includes(option));
    if (reserved !== undefined) return `choice option '${reserved}' is reserved`;
  } else if (!Array.isArray(criteria) || criteria.length < 2) {
    return "score criteria must list at least two levels";
  }
  return Object.values(criteria as object).every(isDescription)
    ? undefined
    : `${type} criteria descriptions must be text, JSON, or null`;
}

/** Why a question map is malformed, or `undefined` when it is valid. */
export function questionsProblem(questions: unknown): string | undefined {
  if (!isRecord(questions)) return "questions must be an object keyed by question id";
  const ids = Object.keys(questions);
  if (ids.length === 0) return "questions must contain at least one question";
  for (const id of ids) {
    if (id.trim() === "") return "question ids must be non-empty";
    if (RESERVED_KEYS.includes(id)) return `question id '${id}' is reserved`;
    const problem = questionProblem(questions[id]);
    if (problem !== undefined) return `question '${id}' ${problem}`;
  }
  return undefined;
}

const within = (value: unknown, min: number, max: number): value is number =>
  typeof value === "number" &&
  Number.isFinite(value) &&
  value >= min - NUMERIC_TOLERANCE &&
  value <= max + NUMERIC_TOLERANCE;

const isProbability = (value: unknown): value is number => within(value, 0, 1);

const isCount = (value: unknown): value is number =>
  typeof value === "number" && Number.isInteger(value) && value >= 0;

function sameKeys(value: Record<string, unknown>, expected: readonly string[]): boolean {
  const keys = Object.keys(value);
  return keys.length === expected.length && expected.every((key) => Object.hasOwn(value, key));
}

/** The fields each answer type carries; normalization keeps exactly these. */
export const ANSWER_FIELDS = {
  yesno: ["type", "yes"],
  refused: ["type"],
  choice: ["type", "choice", "probabilities", "confidence"],
  score: ["type", "score", "legend", "probabilities", "confidence"],
} as const;

function answerProblem(question: AssessmentQuestion, answer: unknown): string | undefined {
  if (!isRecord(answer)) return "must be an object";
  if (answer.type === "refused") {
    return onlyKeys(answer, ANSWER_FIELDS.refused) ? undefined : "has unknown fields";
  }
  if (answer.type !== question.type) return `must have type '${question.type}' or 'refused'`;
  if (!onlyKeys(answer, ANSWER_FIELDS[question.type])) return "has unknown fields";
  if (question.type === "yesno") {
    return isProbability(answer.yes) ? undefined : "yes must be a probability";
  }
  let keys: string[];
  if (question.type === "choice") {
    keys = Object.keys(question.criteria);
    if (typeof answer.choice !== "string" || !keys.includes(answer.choice)) {
      return `choice must be one of ${keys.map((o) => `'${o}'`).join(", ")}`;
    }
  } else {
    keys = question.criteria.map((_, index) => String(index));
    if (!within(answer.score, 0, keys.length - 1)) {
      return `score must be between 0 and ${keys.length - 1}`;
    }
    if (!isRecord(answer.legend) || !sameKeys(answer.legend, keys)) {
      return "legend must cover exactly the defined levels";
    }
  }
  const kind = question.type === "choice" ? "options" : "levels";
  if (!isRecord(answer.probabilities) || !sameKeys(answer.probabilities, keys)) {
    return `probabilities must cover exactly the defined ${kind}`;
  }
  if (!Object.values(answer.probabilities).every(isProbability)) {
    return "probabilities must be numbers between 0 and 1";
  }
  return isProbability(answer.confidence) ? undefined : "confidence must be between 0 and 1";
}

const RESULT_FIELDS = ["provider", "model", "calibrated", "answers", "usage"];

/**
 * Why an assessment output does not answer `questions`, or `undefined` when
 * it does: exactly one answer per question id, of the question's type (or a
 * refusal), within the question's options or levels, plus the provider, the
 * answering model, the calibration flag and token usage.
 */
export function assessmentResultProblem(
  questions: AssessmentQuestions,
  output: unknown,
): string | undefined {
  if (!isRecord(output) || !sameKeys(output, RESULT_FIELDS)) {
    return "output must be { provider, model, calibrated, answers, usage }";
  }
  if (typeof output.provider !== "string" || output.provider.trim() === "") {
    return "provider must be a non-empty string";
  }
  if (typeof output.model !== "string" || output.model.trim() === "") {
    return "model must be a non-empty string";
  }
  if (typeof output.calibrated !== "boolean") return "calibrated must be a boolean";
  const usage = output.usage;
  if (
    !isRecord(usage) ||
    !onlyKeys(usage, ["inputTokens", "outputTokens"]) ||
    (usage.inputTokens !== undefined && !isCount(usage.inputTokens)) ||
    (usage.outputTokens !== undefined && !isCount(usage.outputTokens))
  ) {
    return "usage must be { inputTokens?, outputTokens? } token counts";
  }
  const answers = output.answers;
  const ids = Object.keys(questions);
  if (!isRecord(answers) || !sameKeys(answers, ids)) {
    return `answers must contain exactly the questions ${ids.map((id) => `'${id}'`).join(", ")}`;
  }
  for (const id of ids) {
    const problem = answerProblem(questions[id]!, answers[id]);
    if (problem !== undefined) return `answer '${id}' ${problem}`;
  }
  return undefined;
}

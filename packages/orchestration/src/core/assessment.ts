/**
 * The assessment contract: explicit state evaluated against named, typed
 * questions by a System One model, returning probabilities and confidence.
 * Shapes follow the TypeSafe System One HTTP API
 * (https://docs.typesafe.ai/api): questions are sent verbatim, answers are
 * kept as the API returns them, and token usage is carried in the protocol's
 * camelCase. Jev is the default model family; the contract is not tied to it.
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
export interface NoulQuestion {
  type: "noul";
  instructions: AssessmentEntry;
  criteria?: { true?: AssessmentEntry | null; false?: AssessmentEntry | null };
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

export type AssessmentQuestion = NoulQuestion | ChoiceQuestion | ScoreQuestion;

/** Question ids are chosen by the author; answers come back under the same ids. */
export type AssessmentQuestions = { [id: string]: AssessmentQuestion };

export interface NoulAnswer {
  type: "noul";
  /** Probability of yes, 0 to 1. Noul answers carry no separate confidence. */
  noul: number;
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

export type AssessmentAnswer = NoulAnswer | ChoiceAnswer | ScoreAnswer;

export type AnswerFor<Q> = Q extends { type: "choice"; criteria: infer C }
  ? ChoiceAnswer<Extract<keyof C, string>>
  : Q extends { type: "score" }
    ? ScoreAnswer
    : Q extends { type: "noul" }
      ? NoulAnswer
      : never;

/** Token counts, each present only when the API reported it. */
export interface AssessmentUsage {
  inputTokens?: number;
  outputTokens?: number;
}

/** A succeeded assessment completion's `output`. */
export interface AssessmentResult<Q extends AssessmentQuestions = AssessmentQuestions> {
  /** The versioned model that answered, e.g. `jev-1.13.0` for `jev-latest`. */
  model: string;
  answers: { [K in keyof Q]: AnswerFor<Q[K]> };
  usage: AssessmentUsage;
}

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

const QUESTION_TYPES = ["noul", "choice", "score"];

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
    return "type must be 'choice', 'score', or 'noul'";
  }
  if (!onlyKeys(question, ["type", "instructions", "criteria"])) return "has unknown fields";
  const { type, criteria } = question;
  if (type === "noul") {
    if (criteria === undefined) return undefined;
    if (!isRecord(criteria) || !onlyKeys(criteria, ["true", "false"])) {
      return "noul criteria may only describe 'true' and 'false'";
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

export function isAssessmentQuestions(value: unknown): value is AssessmentQuestions {
  return questionsProblem(value) === undefined;
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
  noul: ["type", "noul"],
  choice: ["type", "choice", "probabilities", "confidence"],
  score: ["type", "score", "legend", "probabilities", "confidence"],
} as const;

function answerProblem(question: AssessmentQuestion, answer: unknown): string | undefined {
  if (!isRecord(answer)) return "must be an object";
  if (answer.type !== question.type) return `must have type '${question.type}'`;
  if (!onlyKeys(answer, ANSWER_FIELDS[question.type])) return "has unknown fields";
  if (question.type === "noul") {
    return isProbability(answer.noul) ? undefined : "noul must be a probability";
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

/**
 * Why an assessment output does not answer `questions`, or `undefined` when
 * it does: exactly one answer per question id, of the question's type, within
 * the question's options or levels, plus the answering model and token usage.
 */
export function assessmentResultProblem(
  questions: AssessmentQuestions,
  output: unknown,
): string | undefined {
  if (!isRecord(output) || !onlyKeys(output, ["model", "answers", "usage"])) {
    return "output must be { model, answers, usage }";
  }
  if (typeof output.model !== "string" || output.model.trim() === "") {
    return "model must be a non-empty string";
  }
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

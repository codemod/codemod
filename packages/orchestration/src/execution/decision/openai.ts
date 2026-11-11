/**
 * OpenAI's Decisions API (`POST /v1/decisions`, model `gpt-6-luna`).
 * Questions are an array of `{ name, type, instructions }`: yes/no is
 * `predicate` (its criteria are appended to the instructions, which take text
 * only), choice options are `choices`, and score levels are `levels` (2 to 10).
 * Answers come back as an array matched by name; a `refusal` becomes
 * `{ type: "refused" }`. Usage reports input tokens only.
 */
import type { AssessmentQuestion } from "../../core/assessment.ts";
import { isRecord } from "../../core/protocol.ts";
import { asText, type DecisionAdapter } from "./adapter.ts";

const MAX_QUESTIONS = 400;
const MAX_LEVELS = 10;

function encodeQuestion(name: string, question: AssessmentQuestion): unknown {
  const instructions = asText(question.instructions);
  if (question.type === "yesno") {
    const { yes, no } = question.criteria ?? {};
    const rubric = [
      yes == null ? "" : `\n\nAnswer yes when: ${asText(yes)}`,
      no == null ? "" : `\n\nAnswer no when: ${asText(no)}`,
    ].join("");
    return { type: "predicate", name, instructions: instructions + rubric };
  }
  if (question.type === "choice") {
    const choices = Object.entries(question.criteria).map(([value, description]) => ({
      value,
      description: description == null ? value : asText(description),
    }));
    return { type: "choice", name, instructions, choices };
  }
  const levels = question.criteria.map((description, index) => ({
    label: String(index),
    description: description == null ? `level ${index}` : asText(description),
  }));
  return { type: "score", name, instructions, levels };
}

const byValue = (entries: unknown, key: string): unknown =>
  Array.isArray(entries) && entries.every((e) => isRecord(e) && key in e && "probability" in e)
    ? Object.fromEntries(
        entries.map((e: Record<string, unknown>) => [String(e[key]), e.probability]),
      )
    : entries;

function decodeAnswer(answer: Record<string, unknown>, question: AssessmentQuestion | undefined) {
  switch (answer.type) {
    case "refusal":
      return { type: "refused" };
    case "predicate":
      return { type: "yesno", yes: answer.probability };
    case "choice":
      return {
        type: "choice",
        choice: answer.choice,
        probabilities: byValue(answer.probabilities, "value"),
        confidence: answer.confidence,
      };
    case "score":
      return {
        type: "score",
        score: answer.score,
        legend:
          question?.type === "score"
            ? Object.fromEntries(question.criteria.map((entry, index) => [String(index), entry]))
            : undefined,
        probabilities: byValue(answer.probabilities, "value"),
        confidence: answer.confidence,
      };
    default:
      return answer;
  }
}

export const openai: DecisionAdapter = {
  encode({ state, questions, model }) {
    const entries = Object.entries(questions);
    if (entries.length > MAX_QUESTIONS) {
      return { problem: `openai decisions take at most ${MAX_QUESTIONS} questions per request` };
    }
    const wide = entries.find(([, q]) => q.type === "score" && q.criteria.length > MAX_LEVELS);
    if (wide !== undefined) {
      return {
        problem: `question '${wide[0]}': openai decisions take at most ${MAX_LEVELS} score levels`,
      };
    }
    const input =
      typeof state === "string"
        ? state
        : [{ role: "user", content: [{ type: "input_json", data: state }] }];
    return {
      body: { model, input, questions: entries.map(([name, q]) => encodeQuestion(name, q)) },
    };
  },
  decode(body, request) {
    if (!isRecord(body) || !Array.isArray(body.answers))
      return { problem: "answers must be an array" };
    const answers: [string, unknown][] = [];
    const seen = new Set<string>();
    for (const answer of body.answers) {
      if (!isRecord(answer) || typeof answer.name !== "string") {
        return { problem: "each answer must be an object with a name" };
      }
      if (seen.has(answer.name)) return { problem: `answers repeat '${answer.name}'` };
      seen.add(answer.name);
      const question = Object.hasOwn(request.questions, answer.name)
        ? request.questions[answer.name]
        : undefined;
      answers.push([answer.name, decodeAnswer(answer, question)]);
    }
    const usage = isRecord(body.usage) ? body.usage : {};
    return {
      model: typeof body.model === "string" ? body.model : request.model,
      answers: Object.fromEntries(answers),
      usage: usage.input_tokens == null ? {} : { inputTokens: usage.input_tokens },
    };
  },
};

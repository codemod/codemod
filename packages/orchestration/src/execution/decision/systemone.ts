/**
 * The System One API (`POST /v1/systemone`): TypeSafe's Jev, Liquid AI d1,
 * open servers that speak it, and Microsoft Decision through Azure AI Foundry
 * (`DECISION_PATH=/providers/microsoft/v1/systemone`, `DECISION_AUTH_HEADER=api-key`,
 * the deployment name as `DECISION_MODEL`). Questions are a map keyed by id,
 * and yes/no questions are its `noul` type.
 */
import type { AssessmentQuestion } from "../../core/assessment.ts";
import { isRecord } from "../../core/protocol.ts";
import type { DecisionAdapter } from "./adapter.ts";

const ANSWER_FIELDS: Record<string, readonly string[]> = {
  choice: ["choice", "probabilities", "confidence"],
  score: ["score", "legend", "probabilities", "confidence"],
};

function encodeQuestion(question: AssessmentQuestion): unknown {
  if (question.type !== "yesno") return question;
  const { criteria } = question;
  return {
    type: "noul",
    instructions: question.instructions,
    ...(criteria === undefined ? {} : { criteria: { true: criteria.yes, false: criteria.no } }),
  };
}

function decodeAnswer(answer: unknown): unknown {
  if (!isRecord(answer)) return answer;
  if (answer.type === "noul") return { type: "yesno", yes: answer.noul };
  const fields = ANSWER_FIELDS[answer.type as string];
  if (fields === undefined || !Object.hasOwn(ANSWER_FIELDS, answer.type as string)) return answer;
  // Only the documented fields reach history; fields the API adds later are dropped.
  return Object.fromEntries(
    ["type", ...fields].filter((f) => Object.hasOwn(answer, f)).map((f) => [f, answer[f]]),
  );
}

export const systemone: DecisionAdapter = {
  encode({ state, questions, model }) {
    const encoded = Object.entries(questions).map(([id, q]) => [id, encodeQuestion(q)]);
    return { body: { state, questions: Object.fromEntries(encoded), model } };
  },
  decode(body) {
    if (!isRecord(body) || !isRecord(body.answers)) return { problem: "answers must be an object" };
    if (body.usage != null && !isRecord(body.usage)) return { problem: "usage must be an object" };
    const usage = body.usage ?? {};
    const counts = [
      ["inputTokens", usage.input_tokens],
      ["outputTokens", usage.output_tokens],
    ].filter(([, count]) => count != null);
    return {
      model: body.model,
      answers: Object.fromEntries(
        Object.entries(body.answers).map(([id, answer]) => [id, decodeAnswer(answer)]),
      ),
      usage: Object.fromEntries(counts),
    };
  },
};

/**
 * A provider adapter translates this package's assessment shapes
 * (`core/assessment.ts`) to one decision API and back. The executor
 * (`execution/assessment.ts`) owns transport, the result's `provider` and
 * `calibrated` fields, and validation of the decoded result.
 */
import type { AssessmentQuestions, AssessmentState } from "../../core/assessment.ts";
import type { DecisionProvider } from "../../core/decision.ts";
import { openai } from "./openai.ts";
import { systemone } from "./systemone.ts";

export interface DecisionRequest {
  state: AssessmentState;
  questions: AssessmentQuestions;
  model: string;
}

export interface DecodedResponse {
  model: unknown;
  answers: { [id: string]: unknown };
  usage: { inputTokens?: unknown; outputTokens?: unknown };
}

export interface DecisionAdapter {
  /** The request body, or why this provider cannot ask these questions. */
  encode(request: DecisionRequest): { body: unknown } | { problem: string };
  /**
   * The response in this package's shapes, or why it cannot be read. Answer
   * values are not checked here; the executor validates the whole result.
   */
  decode(body: unknown, request: DecisionRequest): DecodedResponse | { problem: string };
}

export const DECISION_ADAPTERS: Record<DecisionProvider, DecisionAdapter> = { systemone, openai };

/** JSON entries become text where an API takes text only. */
export const asText = (entry: unknown): string =>
  typeof entry === "string" ? entry : JSON.stringify(entry);

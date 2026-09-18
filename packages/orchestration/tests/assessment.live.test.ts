/**
 * Live `assessment()` against the TypeSafe System One API through the
 * official SDK (`@typesafe-ai/sdk`, default client construction). Opt-in
 * only: ordinary test runs skip it and make no request.
 *
 *   CODEMOD_LIVE_ASSESSMENT=1 TYPESAFE_API_KEY=... \
 *     pnpm --filter @codemod.com/orchestration test:live:assessment
 *
 * Optional: TYPESAFE_BASE_URL (default https://api.typesafe.ai) and
 * TYPESAFE_DEFAULT_MODEL (default jev-latest). Opting in without
 * TYPESAFE_API_KEY fails instead of skipping.
 */
import { beforeAll, describe, expect, it } from "vitest";
import {
  BridgeExecutor,
  MemoryHistoryStore,
  assessment,
  dynamic,
  run,
  type AssessmentResult,
} from "../src/index.ts";

const optedIn = process.env.CODEMOD_LIVE_ASSESSMENT === "1";

describe.skipIf(!optedIn)("assessment() against the live TypeSafe API", () => {
  beforeAll(() => {
    if (!process.env.TYPESAFE_API_KEY?.trim()) {
      throw new Error("CODEMOD_LIVE_ASSESSMENT=1 requires TYPESAFE_API_KEY");
    }
  });

  const buildQuestions = {
    outcome: {
      type: "choice" as const,
      instructions: "Did this build succeed or fail?",
      criteria: { succeeded: "Exit code 0 and no errors", failed: "Non-zero exit or errors" },
    },
    severity: {
      type: "score" as const,
      instructions: "How much work is needed to make this build pass?",
      criteria: ["None", "A small, local fix", "Substantial changes"],
    },
    typeError: {
      type: "noul" as const,
      instructions: "Does the output report a TypeScript type error?",
    },
  };

  const buildReport = assessment({
    name: "build-report",
    ask: () => ({
      state: {
        command: "tsc --noEmit",
        exitCode: 2,
        output: "src/app.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.",
      },
      questions: buildQuestions,
    }),
  });

  it("returns typed answers, the answering model, and usage through run()", async () => {
    const executor = new BridgeExecutor({ bin: "/nonexistent/butterflow-execution-bridge" });
    const store = new MemoryHistoryStore();
    const workflow = dynamic(() => buildReport());
    const { output } = await run(workflow, { executor, history: store });
    const result: AssessmentResult<typeof buildQuestions> = output;

    expect(result.model.trim()).not.toBe("");
    expect(result.usage.inputTokens).toBeGreaterThan(0);
    expect(result.answers.outcome.choice).toBe("failed");
    expect(Object.keys(result.answers.outcome.probabilities).sort()).toEqual([
      "failed",
      "succeeded",
    ]);
    expect(result.answers.outcome.confidence).toBeGreaterThanOrEqual(0);
    expect(result.answers.severity.score).toBeGreaterThanOrEqual(0);
    expect(result.answers.severity.score).toBeLessThanOrEqual(2);
    expect(result.answers.typeError.noul).toBeGreaterThan(0.5);

    // Recorded, so a replay answers without another request.
    const replay = await run(workflow, {
      executor: new BridgeExecutor({
        bin: "/nonexistent/butterflow-execution-bridge",
        assessment: {
          apiKey: "not-used",
          createClient: () => {
            throw new Error("replay must not build a client");
          },
        },
      }),
      history: store,
    });
    expect(replay.output).toEqual(result);
  }, 60_000);
});

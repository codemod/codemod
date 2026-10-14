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
import { mkdirSync, writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import {
  BridgeExecutor,
  MemoryHistoryStore,
  assessment,
  dynamic,
  run,
  type AssessmentFileResult,
} from "../src/index.ts";

const optedIn = process.env.CODEMOD_LIVE_ASSESSMENT === "1";

describe.skipIf(!optedIn)("assessment() against the live TypeSafe API", () => {
  let fixtureDir: string;

  beforeAll(() => {
    if (!process.env.TYPESAFE_API_KEY?.trim()) {
      throw new Error("CODEMOD_LIVE_ASSESSMENT=1 requires TYPESAFE_API_KEY");
    }
    // Create a small fixture with one file to minimize API calls
    fixtureDir = join(tmpdir(), `assessment-live-${process.pid}`);
    rmSync(fixtureDir, { recursive: true, force: true });
    mkdirSync(fixtureDir, { recursive: true });
    writeFileSync(
      join(fixtureDir, "build-output.txt"),
      "src/app.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.",
      "utf8",
    );
  });

  afterAll(() => {
    try {
      rmSync(fixtureDir, { recursive: true, force: true });
    } catch {
      // best effort
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
    include: ["*.txt"],
    ask: () => buildQuestions,
  });

  it("returns typed per-file answers, the answering model, and usage through run()", async () => {
    const spy = vi.spyOn(process, "cwd").mockReturnValue(fixtureDir);
    const executor = new BridgeExecutor({ bin: "/nonexistent/butterflow-execution-bridge" });
    const store = new MemoryHistoryStore();
    const workflow = dynamic(() => buildReport());
    const { output } = await run(workflow, { executor, history: store });

    expect(output).toHaveLength(1);
    const result: AssessmentFileResult<typeof buildQuestions> = output[0]!;

    expect(result.file).toBe("build-output.txt");
    expect(result.assessment.model.trim()).not.toBe("");
    expect(result.assessment.usage.inputTokens).toBeGreaterThan(0);
    expect(result.assessment.answers.outcome.choice).toBe("failed");
    expect(Object.keys(result.assessment.answers.outcome.probabilities).sort()).toEqual([
      "failed",
      "succeeded",
    ]);
    expect(result.assessment.answers.outcome.confidence).toBeGreaterThanOrEqual(0);
    expect(result.assessment.answers.severity.score).toBeGreaterThanOrEqual(0);
    expect(result.assessment.answers.severity.score).toBeLessThanOrEqual(2);
    expect(result.assessment.answers.typeError.noul).toBeGreaterThan(0.5);

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
    expect(replay.output).toEqual(output);

    spy.mockRestore();
  }, 60_000);
});

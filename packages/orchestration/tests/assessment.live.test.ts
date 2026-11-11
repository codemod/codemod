/**
 * Eval: live `assessment()` against real decision APIs. Opt-in only; ordinary
 * runs and PR CI skip it and make no request. Name the providers to ask:
 *
 *   CODEMOD_LIVE_ASSESSMENT=systemone,openai \
 *     DECISION_API_KEY=... (or TYPESAFE_API_KEY / OPENAI_API_KEY) \
 *     pnpm --filter @codemod.com/orchestration test:live:assessment
 *
 * Each provider reads the usual `DECISION_*` settings (`src/core/decision.ts`).
 * A named provider without a key fails instead of skipping.
 */
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  DECISION_PROVIDERS,
  MemoryHistoryStore,
  assessment,
  decisionSettings,
  executeAssessmentOperation,
  run,
  type DecisionProvider,
  type OperationRequest,
} from "../src/index.ts";

const providers = (process.env.CODEMOD_LIVE_ASSESSMENT ?? "")
  .split(",")
  .map((name) => name.trim())
  .filter((name): name is DecisionProvider =>
    (DECISION_PROVIDERS as readonly string[]).includes(name),
  );

const buildReport = assessment({
  name: "build-report",
  include: ["*.txt"],
  ask: () => ({
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
      type: "yesno" as const,
      instructions: "Does the output report a TypeScript type error?",
    },
  }),
});

// With no provider named, one skipped case keeps the file from reporting "no tests".
const cases = providers.length > 0 ? providers : (["systemone"] as const);

describe.skipIf(providers.length === 0).each(cases)(
  "[eval] assessment() against the live %s API",
  (provider) => {
    const env = { ...process.env, DECISION_PROVIDER: provider };
    const executor = (cwd: string) => ({
      execute: (request: OperationRequest, signal?: AbortSignal) =>
        executeAssessmentOperation({ env }, cwd, request, signal),
    });
    let dir: string;
    let previous: string | undefined;

    beforeAll(() => {
      if (decisionSettings(env).apiKey === undefined) {
        throw new Error(`CODEMOD_LIVE_ASSESSMENT=${provider} needs an API key`);
      }
      previous = process.env.DECISION_PROVIDER;
      process.env.DECISION_PROVIDER = provider;
      dir = mkdtempSync(join(tmpdir(), "assessment-live-"));
      writeFileSync(
        join(dir, "build-output.txt"),
        "src/app.ts(3,7): error TS2322: Type 'string' is not assignable to type 'number'.",
      );
    });

    afterAll(() => {
      if (previous === undefined) delete process.env.DECISION_PROVIDER;
      else process.env.DECISION_PROVIDER = previous;
      rmSync(dir, { recursive: true, force: true });
    });

    it("answers every question for the file, and replays without asking again", async () => {
      const store = new MemoryHistoryStore();
      const { output } = await run(buildReport, { executor: executor(dir), history: store });
      const answers = output[0]!.assessment.answers;
      expect({
        provider: output[0]!.assessment.provider,
        outcome: answers.outcome.type === "choice" && answers.outcome.choice,
        typeError: answers.typeError.type === "yesno" && answers.typeError.yes > 0.5,
        severity: answers.severity.type,
      }).toEqual({ provider, outcome: "failed", typeError: true, severity: "score" });

      rmSync(join(dir, "build-output.txt"));
      const replay = await run(buildReport, { executor: executor(dir), history: store });
      expect(replay.output).toEqual(output);
    }, 60_000);
  },
);

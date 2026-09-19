/**
 * The complete assisted workflow. File-oriented assessment evaluates each
 * source file directly; normal TypeScript owns the confidence policy and
 * routes to the deterministic codemod, the agent, or manual review. Every
 * automatic path ends with deterministic verification.
 */
import { dynamic, parallel } from "@codemod.com/orchestration";
import { unwrappedNewCalls } from "./04-parallel.ts";
import { assessSources } from "./08-assessment.ts";
import { migrateWithAgent } from "./09-agent.ts";
import { renameCalls, verifyNoLegacy, wrapOptions } from "./lib/steps.ts";

export default dynamic(async () => {
  const results = await assessSources();

  // Aggregate: if any file is manual or low-confidence, go manual for all
  const anyManual = results.some(
    (r) =>
      r.assessment.answers.route.choice === "manual" ||
      r.assessment.answers.route.confidence < 0.75,
  );

  if (anyManual) {
    return {
      status: "manual-review" as const,
      files: results.map((r) => ({
        file: r.file,
        route: r.assessment.answers.route,
        risk: r.assessment.answers.risk,
        safeToAutomate: r.assessment.answers.safeToAutomate,
      })),
    };
  }

  // Route based on the majority recommendation
  const codemodFiles = results.filter((r) => r.assessment.answers.route.choice === "codemod");
  const useCodemod = codemodFiles.length >= results.length / 2;

  if (useCodemod) {
    const migrations = await renameCalls();
    await wrapOptions({ input: migrations });
  } else {
    await migrateWithAgent();
  }

  const [verification, unwrapped] = await parallel(verifyNoLegacy(), unwrappedNewCalls());
  return {
    status: "completed" as const,
    assessedFiles: results.length,
    verification,
    unwrapped,
  };
});

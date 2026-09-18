/**
 * The complete assisted workflow. Static analysis supplies evidence to a
 * read-only assessment; normal TypeScript owns the confidence policy and
 * routes to the deterministic codemod, the agent, or manual review. Every
 * automatic path ends with deterministic verification.
 */
import { dynamic, parallel } from "@codemod.com/orchestration";
import { legacyCalls, unwrappedNewCalls } from "./04-parallel.ts";
import { assessMigration } from "./08-assessment.ts";
import { migrateWithAgent } from "./09-agent.ts";
import { renameCalls, verifyNoLegacy, wrapOptions } from "./lib/steps.ts";

export default dynamic(async () => {
  const candidates = await legacyCalls();
  const assessment = await assessMigration({ input: candidates });
  const { route, risk, safeToAutomate } = assessment.answers;

  if (route.choice === "manual" || route.confidence < 0.75) {
    return {
      status: "manual-review" as const,
      model: assessment.model,
      route,
      risk,
      safeToAutomate,
      candidates,
    };
  }

  if (route.choice === "codemod") {
    const migrations = await renameCalls();
    await wrapOptions({ input: migrations });
  } else {
    await migrateWithAgent();
  }

  const [verification, unwrapped] = await parallel(verifyNoLegacy(), unwrappedNewCalls());
  return {
    status: "completed" as const,
    model: assessment.model,
    route,
    risk,
    safeToAutomate,
    verification,
    unwrapped,
  };
});

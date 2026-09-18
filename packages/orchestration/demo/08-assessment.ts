/**
 * Deterministic analysis followed by a read-only semantic assessment. JSSG
 * reads the repository; the assessment sees only the explicit findings passed
 * through its input. It returns probabilities and confidence, but changes no
 * files and makes no routing decision.
 *
 * The `ask` function resolves both the model state and the questions from the
 * validated input, so the evidence the model evaluates and the criteria it
 * answers against are always in lockstep.
 */
import { assessment, sequence } from "@codemod.com/orchestration";
import { legacyCalls } from "./04-parallel.ts";
import { Findings } from "./lib/schemas.ts";

export const assessMigration = assessment({
  name: "assess-migration",
  input: Findings,
  ask: (findings) => ({
    state: {
      migration: "oldApi(name) -> newApi({ name })",
      evidence: {
        matchedPattern: "oldApi($ARG)",
        excluded: ["**/*.d.ts", "**/*.generated.ts"],
        candidates: findings.map(({ file, calls }) => ({ file, calls })),
      },
    },
    questions: {
      route: {
        type: "choice" as const,
        instructions: "Which migration route best fits this evidence?",
        criteria: {
          codemod: "A mechanical AST transformation is sufficient",
          agent: "Repository context or coordinated edits are needed",
          manual: "The evidence is insufficient for safe automation",
        },
      },
      risk: {
        type: "score" as const,
        instructions: "How risky is automatic migration?",
        criteria: ["Low", "Moderate", "High", "Manual review required"],
      },
      safeToAutomate: {
        type: "noul" as const,
        instructions: "Is there enough evidence to automate this migration?",
        criteria: {
          true: "The migration can be attempted and verified automatically",
          false: "A person should inspect the candidates before any write",
        },
      },
    },
  }),
});

export default sequence(legacyCalls(), assessMigration());

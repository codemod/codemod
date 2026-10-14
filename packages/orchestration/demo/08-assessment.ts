/**
 * File-oriented read-only assessment. Each TypeScript source file is
 * assessed individually by a System One model. The assessment receives the
 * file's path and content automatically — no JSSG shuttle step is needed.
 *
 * The `ask` function receives `{ file, input }` per file and defines
 * QUESTIONS ONLY. The model state is assembled automatically from the
 * file's path, content, and optional workflow input.
 */
import { assessment } from "@codemod.com/orchestration";

export const assessSources = assessment({
  name: "assess-sources",
  include: ["src/**/*.ts"],
  exclude: ["**/*.d.ts", "**/*.generated.ts"],
  ask: ({ file }) => ({
    route: {
      type: "choice" as const,
      instructions: `Which migration route best fits ${file.path}?`,
      criteria: {
        codemod: "A mechanical AST transformation is sufficient",
        agent: "Repository context or coordinated edits are needed",
        manual: "The evidence is insufficient for safe automation",
      },
    },
    risk: {
      type: "score" as const,
      instructions: "How risky is automatic migration of this file?",
      criteria: ["Low", "Moderate", "High", "Manual review required"],
    },
    safeToAutomate: {
      type: "noul" as const,
      instructions: "Is there enough evidence to automate this file's migration?",
      criteria: {
        true: "The migration can be attempted and verified automatically",
        false: "A person should inspect the file before any write",
      },
    },
  }),
});

/**
 * Run the assessment as a standalone workflow. Assessment is a first-class
 * Runnable — it can be the root directly, no `dynamic()` wrapper needed.
 * The result is an ordered array of `{ file, assessment }` entries in
 * deterministic file order.
 */
export default assessSources;

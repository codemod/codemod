/**
 * An agent performs the contextual edit, then ordinary code verifies the
 * repository. The agent uses the installed Claude Code login, receives only
 * file tools, and cannot invoke a shell. The sequence returns the verifier's
 * result rather than treating the agent's final message as proof.
 */
import { agent, sequence } from "@codemod.com/orchestration";
import { verifyNoLegacy } from "./lib/steps.ts";

export const migrateWithAgent = agent({
  name: "agent-migration",
  backend: {
    kind: "claude-code",
    tools: ["Read", "Glob", "Grep", "Edit", "Write"],
  },
  prompt: [
    "Migrate oldApi(name) to newApi({ name }) in TypeScript files under src.",
    "Do not modify declaration files or generated files.",
    "Do not use a shell. Make only the source edits required by this migration.",
  ].join("\n"),
});

export default sequence(migrateWithAgent(), verifyNoLegacy());

/**
 * Live builtin `agent()`: the real bridge and `codemod-ai` (Rig). Opt-in only;
 * ordinary test runs skip it and make no request.
 *
 *   LLM_API_KEY=... pnpm --filter @codemod.com/orchestration test:live:agent:builtin
 *   # = pnpm build:bridge && CODEMOD_LIVE_AGENT_BUILTIN=1 vitest run tests/agent.builtin.live.test.ts
 *
 * Optional: LLM_PROVIDER (openai | anthropic | google_ai | azure_openai,
 * default openai), LLM_MODEL (default gpt-4o), LLM_BASE_URL (default per
 * provider), CODEMOD_BRIDGE_BIN. Opting in without LLM_API_KEY fails.
 */
import { liveAgentSuite } from "./live-agent.ts";

liveAgentSuite({
  title: "agent() with the builtin backend against a live model",
  flag: "CODEMOD_LIVE_AGENT_BUILTIN",
  backend: { kind: "builtin" },
  // Builtin file tools accept any absolute path; only the target write is asserted.
  confined: false,
  precondition: () => {
    if (!process.env.LLM_API_KEY?.trim()) {
      throw new Error("CODEMOD_LIVE_AGENT_BUILTIN=1 requires LLM_API_KEY");
    }
  },
});

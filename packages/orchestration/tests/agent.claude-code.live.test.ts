/**
 * Live `claude-code` `agent()`: the real bridge and the installed, logged-in
 * `claude` CLI (Claude Code subscription quota; not codemod-ai/Rig). Opt-in
 * only; ordinary test runs skip it and start nothing.
 *
 *   pnpm --filter @codemod.com/orchestration test:live:agent:claude-code
 *   # = pnpm build:bridge && CODEMOD_LIVE_AGENT_CLAUDE_CODE=1 vitest run tests/agent.claude-code.live.test.ts
 *
 * Requires `claude` on PATH and `claude auth login` done. The login check
 * reads only `loggedIn` from `claude auth status --json`, run with the
 * bridge's filtered environment from an empty private directory. One call
 * with the default claude-code tools (Read, Edit, Write, Glob, Grep; no Bash)
 * and no bypass flags; it also checks that a shell command is denied, a write
 * outside the target is refused, and a project `.claude/settings.json`
 * (`apiKeyHelper`, hook) is not loaded. Opting in with the CLI missing or
 * logged out fails.
 */
import { liveAgentSuite, loginState } from "./live-agent.ts";

liveAgentSuite({
  title: "agent() with the claude-code backend against the installed CLI",
  flag: "CODEMOD_LIVE_AGENT_CLAUDE_CODE",
  backend: { kind: "claude-code" },
  confined: true,
  precondition: () => {
    const state = loginState("claude-code", "claude", ["auth", "status", "--json"], (stdout) => {
      try {
        return (JSON.parse(stdout) as { loggedIn?: unknown }).loggedIn === true;
      } catch {
        return false;
      }
    });
    if (state === "missing") throw new Error("claude-code live test requires `claude` on PATH");
    if (state === "logged-out") {
      throw new Error("claude-code live test requires a Claude Code login (`claude auth login`)");
    }
  },
});

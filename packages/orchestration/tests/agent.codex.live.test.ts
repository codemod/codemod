/**
 * Live `codex` `agent()`: the real bridge and the installed, logged-in `codex`
 * CLI (Codex subscription quota; not codemod-ai/Rig). Opt-in only; ordinary
 * test runs skip it and start nothing.
 *
 *   pnpm --filter @codemod.com/orchestration test:live:agent:codex
 *   # = pnpm build:bridge && CODEMOD_LIVE_AGENT_CODEX=1 vitest run tests/agent.codex.live.test.ts
 *
 * Requires `codex` on PATH and `codex login` done. The login check uses only
 * the exit code of `codex login status`, run with the bridge's filtered
 * environment from an empty private directory. Two calls: a `workspace-write`
 * step (also checks a shell command and a write outside the target are
 * blocked and a project `.codex/config.toml` is ignored) and a `read-only`
 * step whose write must fail. Approvals never ask; no bypass flags. Opting in
 * with the CLI missing or logged out fails.
 */
import { liveAgentSuite, loginState } from "./live-agent.ts";

liveAgentSuite({
  title: "agent() with the codex backend against the installed CLI",
  flag: "CODEMOD_LIVE_AGENT_CODEX",
  backend: { kind: "codex", sandbox: "workspace-write" },
  confined: true,
  readOnly: { kind: "codex", sandbox: "read-only" },
  precondition: () => {
    const state = loginState("codex", "codex", ["login", "status"], () => true);
    if (state === "missing") throw new Error("codex live test requires `codex` on PATH");
    if (state === "logged-out") {
      throw new Error("codex live test requires a Codex login (`codex login`)");
    }
  },
});

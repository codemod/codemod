/**
 * Where the `node` and `quickjs` runtimes find the `codemod-agent` runner
 * (`crates/orchestration-agent`): `CODEMOD_AGENT_BIN`, else the newer of the
 * monorepo's `target/{release,debug}/codemod-agent` (honouring
 * `CARGO_TARGET_DIR`). Only an `agent` step needs it; the path is checked
 * when one runs (`execution/agent.ts`).
 */
import { existsSync, statSync } from "node:fs";
import { isAbsolute, join, resolve } from "node:path";
import { AGENT_BINARY, AGENT_BIN_ENV } from "../../execution/agent.ts";

const REPO = resolve(import.meta.dirname, "../../../../..");

export function defaultAgentPath(env: Record<string, string | undefined> = process.env): string {
  const explicit = env[AGENT_BIN_ENV];
  if (explicit !== undefined && explicit.trim() !== "") return resolve(explicit);
  const target =
    env.CARGO_TARGET_DIR === undefined
      ? join(REPO, "target")
      : isAbsolute(env.CARGO_TARGET_DIR)
        ? env.CARGO_TARGET_DIR
        : join(REPO, env.CARGO_TARGET_DIR);
  const candidates = ["release", "debug"]
    .map((profile) => join(target, profile, AGENT_BINARY))
    .filter((path) => existsSync(path))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
  return candidates[0] ?? join(target, "debug", AGENT_BINARY);
}

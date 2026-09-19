/**
 * The environment an `agent` bridge starts with. An agent's tools run
 * commands chosen by a model, so the host environment is not inherited: only
 * what a process needs to start, reach the network, and configure the agent,
 * plus variables the host passed explicitly (`BridgeOptions.env`).
 * Everything else (TypeSafe keys, cloud and CI credentials, tokens) stays out.
 *
 * `LLM_API_KEY` is allowlisted only so it can be found: `agentLaunch` moves
 * it out of the launch environment and onto the bridge's stdin
 * (`CODEMOD_BRIDGE_SECRETS=stdin`), so it is not visible through `ps eww` or
 * `/proc/<pid>/environ` of the bridge. The bridge also removes it from its own
 * environment before the agent starts.
 *
 * External backends (`claude-code`, `codex`) get no `LLM_*` variables and no
 * stdin secret: the CLI authenticates with its own local login, found through
 * `HOME` or a non-default `CLAUDE_CONFIG_DIR` / `CODEX_HOME`. Provider API key
 * variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, ...) are not allowlisted.
 *
 * This limits inheritance and accidental exposure only. Tools that run
 * commands (`bash`, `mcp_tool`) run as the same user and can read that user's
 * files and, where the OS allows, other processes' environments, including
 * the host's.
 */

import type { AgentBackendKind } from "../core/protocol.ts";

/** Exact names, compared case-insensitively (Windows environments are). */
export const AGENT_ENV_ALLOWLIST: readonly string[] = [
  // process basics
  "PATH",
  "HOME",
  "USER",
  "LOGNAME",
  "SHELL",
  "TMPDIR",
  "TMP",
  "TEMP",
  "LANG",
  "LANGUAGE",
  "TERM",
  "TZ",
  // Windows process basics
  "SYSTEMROOT",
  "WINDIR",
  "COMSPEC",
  "PATHEXT",
  "SYSTEMDRIVE",
  "USERPROFILE",
  "APPDATA",
  "LOCALAPPDATA",
  "PROGRAMDATA",
  // network: model endpoints behind proxies or private CAs
  "HTTP_PROXY",
  "HTTPS_PROXY",
  "NO_PROXY",
  "ALL_PROXY",
  "SSL_CERT_FILE",
  "SSL_CERT_DIR",
  // external harness configuration: where an installed CLI keeps its local
  // login and settings when it is not the default under HOME (no credentials)
  "CLAUDE_CONFIG_DIR",
  "CODEX_HOME",
  // builtin agent configuration
  "LLM_API_KEY",
  "LLM_PROVIDER",
  "LLM_MODEL",
  "LLM_BASE_URL",
];

/** Name prefixes, also case-insensitive: locale categories. */
export const AGENT_ENV_PREFIXES: readonly string[] = ["LC_"];

const allowed = new Set(AGENT_ENV_ALLOWLIST);

/**
 * Allowlisted host variables plus `explicit`, which wins. On Windows names are
 * case-insensitive, so `Path` from `explicit` replaces a host `PATH` instead of
 * producing both.
 */
export function agentEnvironment(
  host: Record<string, string | undefined>,
  explicit: Record<string, string> = {},
  platform: NodeJS.Platform = process.platform,
): Record<string, string> {
  const key = (name: string) => (platform === "win32" ? name.toUpperCase() : name);
  const merged = new Map<string, [string, string]>();
  for (const [name, value] of Object.entries(host)) {
    if (value === undefined) continue;
    const upper = name.toUpperCase();
    if (allowed.has(upper) || AGENT_ENV_PREFIXES.some((prefix) => upper.startsWith(prefix))) {
      merged.set(key(name), [name, value]);
    }
  }
  for (const [name, value] of Object.entries(explicit)) merged.set(key(name), [name, value]);
  return Object.fromEntries(merged.values());
}

/** Name tokens that mark a variable as a credential. Mirrors `external.rs`. */
const SECRET_TOKENS = new Set([
  "KEY",
  "KEYS",
  "APIKEY",
  "TOKEN",
  "TOKENS",
  "SECRET",
  "SECRETS",
  "PASSWORD",
  "PASSWD",
  "CREDENTIAL",
  "CREDENTIALS",
  "AUTH",
  "OAUTH",
  "COOKIE",
  "SESSION",
  "PRIVATE",
]);

/** Provider and agent prefixes that route or authenticate model access. Mirrors `external.rs`. */
const PROVIDER_PREFIXES = [
  "ANTHROPIC_",
  "OPENAI_",
  "AZURE_",
  "AWS_",
  "GOOGLE_",
  "GEMINI_",
  "GCP_",
  "VERTEX_",
  "BEDROCK_",
  "CLAUDE_CODE_",
  "CODEX_",
  "LLM_",
];

/** Non-secret locations of an external CLI's own login and settings. */
export const EXTERNAL_ENV_ALLOWED: readonly string[] = ["CLAUDE_CONFIG_DIR", "CODEX_HOME"];

/**
 * Whether a variable name looks like a credential or a provider setting that
 * must not reach an external CLI or the tools it runs (`ANTHROPIC_API_KEY`,
 * `GITHUB_TOKEN`, `AWS_PROFILE`, `CODEX_API_KEY`, ...). `CLAUDE_CONFIG_DIR`
 * and `CODEX_HOME` are allowed.
 */
export function isSecretEnvName(name: string): boolean {
  const upper = name.toUpperCase();
  if (EXTERNAL_ENV_ALLOWED.includes(upper)) return false;
  return (
    PROVIDER_PREFIXES.some((prefix) => upper.startsWith(prefix)) ||
    upper.split(/[^A-Z0-9]/u).some((token) => SECRET_TOKENS.has(token))
  );
}

/**
 * External backends authenticate with the CLI's own login. An explicitly
 * passed variable that looks like a credential is refused instead of being
 * handed to Claude Code's `Bash` or Codex's shell; returns the reason, or
 * undefined when every explicit name is acceptable.
 */
export function externalAgentEnvProblem(explicit: Record<string, string> = {}): string | undefined {
  const refused = Object.keys(explicit).filter(isSecretEnvName).sort();
  if (refused.length === 0) return undefined;
  return `external agent backends do not accept credential or provider variables in BridgeOptions.env (${refused.join(", ")}); they use the CLI's own login`;
}

/** Marker telling the bridge to read `{ "LLM_API_KEY": ... }` from stdin. */
export const BRIDGE_SECRETS_ENV = "CODEMOD_BRIDGE_SECRETS";

/**
 * How an agent bridge is launched: the allowlisted environment without
 * `LLM_API_KEY`, and the key as JSON for the bridge's stdin when there is one.
 */
export function agentLaunch(
  host: Record<string, string | undefined>,
  explicit: Record<string, string> = {},
  platform: NodeJS.Platform = process.platform,
  backend: AgentBackendKind = "builtin",
): { env: Record<string, string>; stdin?: string } {
  const env = agentEnvironment(host, explicit, platform);
  let apiKey: string | undefined;
  for (const name of Object.keys(env)) {
    const upper = name.toUpperCase();
    if (backend !== "builtin" && isSecretEnvName(name)) {
      // Defense in depth: `externalAgentEnvProblem` refuses these first.
      delete env[name];
      continue;
    }
    if (upper !== "LLM_API_KEY") continue;
    // On POSIX an exact-case name wins over a lookalike; Windows kept only one.
    if (name === "LLM_API_KEY" || apiKey === undefined) apiKey = env[name];
    delete env[name];
  }
  if (apiKey === undefined || apiKey.trim() === "") return { env };
  return {
    env: { ...env, [BRIDGE_SECRETS_ENV]: "stdin" },
    stdin: JSON.stringify({ LLM_API_KEY: apiKey }),
  };
}

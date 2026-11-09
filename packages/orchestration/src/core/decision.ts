/**
 * Which decision model answers assessments, read from the environment only:
 * workflow code never names a provider, endpoint, or key. The QuickJS host
 * resolves the same base URL to scope its network allowlist
 * (`crates/orchestration-quickjs-host/src/http.rs`); keep the two in step.
 *
 * | Variable               | Meaning                                              |
 * | ---------------------- | ---------------------------------------------------- |
 * | `DECISION_PROVIDER`    | `systemone` (default) or `openai`                    |
 * | `DECISION_BASE_URL`    | API root; the provider's default when unset          |
 * | `DECISION_PATH`        | request path under the root; the provider's default  |
 * | `DECISION_MODEL`       | model or deployment name; the provider's default     |
 * | `DECISION_API_KEY`     | the key                                              |
 * | `DECISION_AUTH_HEADER` | `authorization` (default, sends `Bearer <key>`) or a |
 * |                        | header that carries the raw key, such as `api-key`   |
 * | `DECISION_CALIBRATED`  | `true` or `false`: whether answers are calibrated    |
 * |                        | probabilities (see below)                            |
 *
 * `systemone` still reads `TYPESAFE_BASE_URL`, `TYPESAFE_DEFAULT_MODEL` and
 * `TYPESAFE_API_KEY` when the `DECISION_*` value is unset, and `openai` reads
 * `OPENAI_API_KEY`. Blank values count as unset.
 *
 * Calibration is a property of the deployment, not the wire format: TypeSafe
 * and OpenAI document calibrated probabilities, while Microsoft-Decision-1 on
 * Foundry (a path under `/providers/microsoft/`) documents its scores only for
 * ordering and thresholds. `DECISION_CALIBRATED` overrides either default.
 */

export const DECISION_PROVIDERS = ["systemone", "openai"] as const;
export type DecisionProvider = (typeof DECISION_PROVIDERS)[number];

export interface DecisionSettings {
  provider: DecisionProvider;
  /** `baseURL` joined with the path. */
  url: string;
  /** Unset when no key variable is set; only a live request needs one. */
  apiKey?: string;
  /** Lower-case header name; `authorization` sends `Bearer <key>`. */
  authHeader: string;
  /** Whether answers are calibrated probabilities; recorded in each result. */
  calibrated: boolean;
}

const DEFAULTS: Record<
  DecisionProvider,
  { baseURL: string; path: string; model: string; legacy: { [setting: string]: string } }
> = {
  systemone: {
    baseURL: "https://api.typesafe.ai",
    path: "/v1/systemone",
    model: "jev-latest",
    legacy: {
      baseURL: "TYPESAFE_BASE_URL",
      model: "TYPESAFE_DEFAULT_MODEL",
      apiKey: "TYPESAFE_API_KEY",
    },
  },
  openai: {
    baseURL: "https://api.openai.com",
    path: "/v1/decisions",
    model: "gpt-6-luna",
    legacy: { apiKey: "OPENAI_API_KEY" },
  },
};

type Env = Readonly<Record<string, string | undefined>>;

const read = (env: Env, name: string | undefined) =>
  name === undefined ? undefined : env[name]?.trim() || undefined;

/** The configured provider; throws when `DECISION_PROVIDER` names an unknown one. */
export function decisionProvider(env: Env): DecisionProvider {
  const provider = read(env, "DECISION_PROVIDER") ?? "systemone";
  if (!(DECISION_PROVIDERS as readonly string[]).includes(provider)) {
    throw new Error(
      `DECISION_PROVIDER '${provider}' is not one of ${DECISION_PROVIDERS.join(", ")}`,
    );
  }
  return provider as DecisionProvider;
}

const setting = (
  env: Env,
  provider: DecisionProvider,
  name: "baseURL" | "model" | "apiKey",
  variable: string,
) => read(env, variable) ?? read(env, DEFAULTS[provider].legacy[name]);

/** Which provider and model answer, as recorded in history. Needs no endpoint or key. */
export function decisionModel(env: Env): { provider: DecisionProvider; model: string } {
  const provider = decisionProvider(env);
  return {
    provider,
    model: setting(env, provider, "model", "DECISION_MODEL") ?? DEFAULTS[provider].model,
  };
}

/**
 * Resolve the settings for `provider` (default: `DECISION_PROVIDER`). Throws
 * on an unknown provider or a base URL that is not plain http(s).
 */
export function decisionSettings(env: Env, provider = decisionProvider(env)): DecisionSettings {
  const defaults = DEFAULTS[provider];
  const baseURL = setting(env, provider, "baseURL", "DECISION_BASE_URL") ?? defaults.baseURL;
  let base: URL;
  try {
    base = new URL(baseURL);
  } catch {
    throw new Error(`decision base URL '${baseURL}' is not a URL`);
  }
  if (base.protocol !== "https:" && base.protocol !== "http:") {
    throw new Error(`decision base URL '${baseURL}' is not http(s)`);
  }
  if (base.search !== "" || base.hash !== "" || base.username !== "" || base.password !== "") {
    throw new Error(`decision base URL '${baseURL}' must not carry a query, fragment, or user`);
  }
  const path = read(env, "DECISION_PATH") ?? defaults.path;
  const settings: DecisionSettings = {
    provider,
    url: `${baseURL.replace(/\/+$/, "")}/${path.replace(/^\/+/, "")}`,
    authHeader: (read(env, "DECISION_AUTH_HEADER") ?? "authorization").toLowerCase(),
    calibrated: calibrated(env, path),
  };
  const apiKey = setting(env, provider, "apiKey", "DECISION_API_KEY");
  if (apiKey !== undefined) settings.apiKey = apiKey;
  return settings;
}

function calibrated(env: Env, path: string): boolean {
  const override = read(env, "DECISION_CALIBRATED")?.toLowerCase();
  if (override === "true") return true;
  if (override === "false") return false;
  if (override !== undefined) {
    throw new Error(`DECISION_CALIBRATED '${override}' is not true or false`);
  }
  return !/(^|\/)providers\/microsoft\//u.test(path);
}

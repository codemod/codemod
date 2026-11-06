/**
 * `bridge` (default): the control plane in this Node process; `shell`,
 * `jssg`, and `agent` in one `butterflow-execution-bridge` process each;
 * `assessment` from this process. Every operation kind is supported.
 */
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { BridgeExecutor } from "../../execution/executor.ts";
import { BRIDGE_RUNTIME } from "../../runtime/capabilities.ts";
import type { RuntimeProvider } from "./types.ts";

/** `CODEMOD_BRIDGE_BIN`, else the monorepo debug build when running from the source checkout. */
export function defaultBridgePath(env: Record<string, string | undefined> = process.env): string {
  return (
    env.CODEMOD_BRIDGE_BIN ??
    resolve(import.meta.dirname, "../../../../../target/debug/butterflow-execution-bridge")
  );
}

export const bridgeRuntime: RuntimeProvider = {
  ...BRIDGE_RUNTIME,
  description: "control plane in Node; each shell/jssg/agent operation in a Rust bridge process",
  availability(settings) {
    if (existsSync(settings.bridge)) return { available: true };
    return {
      available: false,
      reason: `bridge binary not found at ${settings.bridge}; build it with 'cargo build -p butterflow-execution-bridge' or pass --bridge / CODEMOD_BRIDGE_BIN`,
    };
  },
  createExecutor: ({ cwd, artifacts, events, settings }) =>
    new BridgeExecutor({
      bin: settings.bridge,
      cwd,
      artifacts,
      ...(events === undefined ? {} : { events }),
      ...(settings.env === undefined ? {} : { env: settings.env }),
    }),
};

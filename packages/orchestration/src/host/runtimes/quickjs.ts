/**
 * `quickjs`: the whole workflow (control plane, workflow body, every
 * transform) in one QuickJS context hosted by a Rust binary
 * (`crates/orchestration-quickjs-host`); no Node at runtime. `launch` builds
 * the workflow bundle (`host/quickjs-bundle.ts`, cached by the content of
 * every input file) and runs the host binary on it. `agent`, `assessment`,
 * and JSSG semantic analysis are refused by the capability contract inside
 * the bundle's control plane.
 *
 * The binary: `CODEMOD_QUICKJS_HOST_BIN`, else the newer of the monorepo's
 * `target/{release,debug}/orchestration-quickjs-host` (honouring
 * `CARGO_TARGET_DIR`).
 */
import { spawn } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { isAbsolute, join, resolve } from "node:path";
import { Phase, lap, now } from "../../core/timings.ts";
import { QUICKJS_RUNTIME } from "../../runtime/capabilities.ts";
import { timeChildProcess } from "../timings-file.ts";
import type { RuntimeProvider } from "./types.ts";

export const QUICKJS_HOST_BINARY = "orchestration-quickjs-host";

const REPO = resolve(import.meta.dirname, "../../../../..");

/** Where the host binary is expected (it may not exist). */
export function defaultQuickjsHostPath(
  env: Record<string, string | undefined> = process.env,
): string {
  if (env.CODEMOD_QUICKJS_HOST_BIN !== undefined) return resolve(env.CODEMOD_QUICKJS_HOST_BIN);
  const target =
    env.CARGO_TARGET_DIR === undefined
      ? join(REPO, "target")
      : isAbsolute(env.CARGO_TARGET_DIR)
        ? env.CARGO_TARGET_DIR
        : join(REPO, env.CARGO_TARGET_DIR);
  const candidates = ["release", "debug"]
    .map((profile) => join(target, profile, QUICKJS_HOST_BINARY))
    .filter((path) => existsSync(path))
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
  return candidates[0] ?? join(target, "debug", QUICKJS_HOST_BINARY);
}

interface HostResult {
  ok: boolean;
  output?: unknown;
  error?: string;
}

/** Run a prebuilt bundle on the host binary; resolves with the workflow output. */
export function runQuickjsBundle(options: {
  host: string;
  bundle: string;
  target: string;
  input?: unknown;
  hasInput: boolean;
  env?: Record<string, string>;
  signal?: AbortSignal;
}): Promise<unknown> {
  return new Promise((resolvePromise, reject) => {
    const args = ["run", options.bundle, "--target", options.target, "--json"];
    if (options.hasInput) args.push("--input", JSON.stringify(options.input ?? null));
    // Benchmark timings (no-op when off): the host writes its own file.
    const timed = timeChildProcess();
    const child = spawn(options.host, args, {
      env: { ...process.env, ...options.env, ...timed.env },
      // stderr carries the workflow's console output.
      stdio: ["ignore", "pipe", "inherit"],
    });
    const chunks: Buffer[] = [];
    child.stdout.on("data", (chunk: Buffer) => chunks.push(chunk));
    // The host turns SIGTERM into an abort of the run (shell children are in
    // their own process groups and are killed by that abort).
    const onAbort = () => child.kill("SIGTERM");
    if (options.signal?.aborted) onAbort();
    else options.signal?.addEventListener("abort", onAbort, { once: true });
    child.once("error", (error) => {
      options.signal?.removeEventListener("abort", onAbort);
      reject(error);
    });
    child.once("close", (code) => {
      options.signal?.removeEventListener("abort", onAbort);
      timed.exited(child.pid);
      const text = Buffer.concat(chunks).toString("utf8");
      let result: HostResult | undefined;
      try {
        result = JSON.parse(text) as HostResult;
      } catch {
        result = undefined;
      }
      if (result?.ok === true) resolvePromise(result.output);
      else if (result !== undefined) reject(new Error(result.error ?? "the workflow failed"));
      else reject(new Error(`${QUICKJS_HOST_BINARY} exited with code ${code} without a result`));
    });
  });
}

export const quickjsRuntime: RuntimeProvider = {
  ...QUICKJS_RUNTIME,
  description: "whole prebuilt workflow bundle in one QuickJS context hosted by a Rust binary",
  availability() {
    const host = defaultQuickjsHostPath();
    if (existsSync(host)) return { available: true };
    return {
      available: false,
      reason: `quickjs host binary not found at ${host}; build it with 'cargo build -p orchestration-quickjs-host' (or --release) or set CODEMOD_QUICKJS_HOST_BIN`,
    };
  },
  async launch(request) {
    // Loaded on demand: esbuild and TypeScript load only on a cache miss.
    // Benchmark timings (no-op when off): finding or building the bundle is `plan`.
    const planned = now();
    const { cachedQuickjsBundle } = await import("../quickjs-bundle.ts");
    const bundle = await cachedQuickjsBundle(request.workflow);
    lap(Phase.plan, planned);
    return runQuickjsBundle({
      host: defaultQuickjsHostPath(),
      bundle: bundle.path,
      target: request.target,
      input: request.input,
      hasInput: "input" in request,
      ...(request.settings.env === undefined ? {} : { env: request.settings.env }),
      ...(request.signal === undefined ? {} : { signal: request.signal }),
    });
  },
};

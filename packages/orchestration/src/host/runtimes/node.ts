/**
 * `node`: the whole workflow in this Node process. JSSG transforms run
 * in-process (each artifact module evaluated once per run, cached by hash)
 * over the napi-rs ast-grep addon (`@codemod.com/jssg-napi`), through the
 * portable orchestrator (`execution/jssg.ts::executeJssgCommand` with the
 * portable batch loop of `execution/jssg-batch.ts`, whose batch rules are
 * the bridge's own Rust through the addon's `JssgBatch`); `shell` runs
 * through the host (`execution/shell.ts::executeShell`); `assessment` is one
 * TypeSafe SDK call per request from this process; JSSG semantic analysis
 * through the batch's provider. `agent` is refused by the capability
 * contract (`NODE_RUNTIME`) before anything executes. The executor is
 * `node-executor.ts`, imported by `load()`.
 *
 * Parsing (and the static selector test) happens on worker threads, a chunk
 * ahead of the transforms; every transform runs on this one JS thread,
 * serially per file and one at a time across concurrent commands.
 * Module-level state in a transform is unsupported: it lives for one run and
 * is shared by every file of every command that uses the artifact.
 */
import { bindingPath, isAvailable } from "@codemod.com/jssg-napi";
import { NODE_RUNTIME } from "../../runtime/capabilities.ts";
import { notLoaded, type RuntimeProvider } from "./types.ts";

let executor: typeof import("./node-executor.ts") | undefined;

export const nodeRuntime: RuntimeProvider = {
  ...NODE_RUNTIME,
  description: "whole workflow in one Node process; transforms in-process over a napi addon",
  availability() {
    if (isAvailable()) return { available: true };
    return {
      available: false,
      reason: `runtime 'node' needs the jssg napi addon, not found at ${bindingPath()}; build it with 'pnpm --filter @codemod.com/jssg-napi build' or set CODEMOD_JSSG_NAPI`,
    };
  },
  async load() {
    executor ??= await import("./node-executor.ts");
  },
  createExecutor({ cwd, artifacts, settings }) {
    if (executor === undefined) throw notLoaded("node");
    return new executor.NodeExecutor({
      cwd,
      artifacts,
      ...(settings.env === undefined ? {} : { env: settings.env }),
    });
  },
};

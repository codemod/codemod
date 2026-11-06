/**
 * `node`: the whole workflow in this Node process. JSSG transforms run
 * in-process (each artifact module evaluated once per run, cached by hash)
 * over the napi-rs ast-grep addon, through the portable orchestrator
 * (`execution/jssg.ts::executeJssgCommand` with an in-process `dispatch`);
 * `shell` runs through the host (`execution/shell.ts::executeShell`).
 * `agent`, `assessment`, and JSSG semantic analysis are refused by the
 * capability contract (`NODE_RUNTIME`).
 *
 * Placeholder: the executor lands with the napi addon. Replace this file's
 * `availability` and add `createExecutor`; nothing else changes.
 */
import { NODE_RUNTIME } from "../../runtime/capabilities.ts";
import { notAvailableYet, type RuntimeProvider } from "./types.ts";

export const nodeRuntime: RuntimeProvider = {
  ...NODE_RUNTIME,
  description: "whole workflow in one Node process; transforms in-process over a napi addon",
  availability: () => notAvailableYet("node"),
};

/**
 * `quickjs`: the whole workflow (control plane, workflow body, every
 * transform) in one QuickJS context hosted by a Rust binary; no Node at
 * runtime. The workflow is prebuilt by Node into one bundle (`codemod-workflow
 * build`, esbuild) whose entry installs the QuickJS `Host` (see `RUNTIMES.md`)
 * and calls `run(workflow, { executor, runtime: QUICKJS_RUNTIME })` from
 * `@codemod.com/orchestration/portable`. `agent`, `assessment`, and JSSG
 * semantic analysis are refused by the capability contract.
 *
 * Placeholder: `launch` (build the bundle, spawn the host binary, return its
 * output) lands with the binary. Replace this file; nothing else changes.
 */
import { QUICKJS_RUNTIME } from "../../runtime/capabilities.ts";
import { notAvailableYet, type RuntimeProvider } from "./types.ts";

export const quickjsRuntime: RuntimeProvider = {
  ...QUICKJS_RUNTIME,
  description: "whole prebuilt workflow bundle in one QuickJS context hosted by a Rust binary",
  availability: () => notAvailableYet("quickjs"),
};

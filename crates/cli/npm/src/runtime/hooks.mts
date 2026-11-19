// Resolve and load hooks for Node.js versions without `module.registerHooks`
// (loaded through `module.register`, on the loader thread).

import type { LoadHook, ResolveHook } from "node:module";

import { resolveCodemodModule } from "./modules.mts";
import { loadWorkflow } from "./transforms.mts";

export const resolve: ResolveHook = async (specifier, context, nextResolve) => {
  const url = resolveCodemodModule(specifier);
  return url ? { url, shortCircuit: true } : nextResolve(specifier, context);
};

/** The workflow file the host imports, with its imported transforms replaced. */
export const load: LoadHook = async (url, context, nextLoad) =>
  loadWorkflow(url, await nextLoad(url, context));

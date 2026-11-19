// Resolve hooks for Node.js versions without `module.registerHooks`
// (loaded through `module.register`, on the loader thread).

import type { ResolveHook } from "node:module";

import { resolveCodemodModule } from "./modules.mts";

export const resolve: ResolveHook = async (specifier, context, nextResolve) => {
  const url = resolveCodemodModule(specifier);
  return url ? { url, shortCircuit: true } : nextResolve(specifier, context);
};

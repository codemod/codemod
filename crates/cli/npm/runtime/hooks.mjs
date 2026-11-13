// Resolve hooks for Node.js versions without `module.registerHooks`
// (loaded through `module.register`, on the loader thread).

import { resolveCodemodModule } from "./modules.mjs";

export async function resolve(specifier, context, nextResolve) {
  const url = resolveCodemodModule(specifier);
  return url ? { url, shortCircuit: true } : nextResolve(specifier, context);
}

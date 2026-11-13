// `import "codemod/register"`: lets Node.js and Bun import workflows, which
// import `codemod:workflow`, `codemod:ast-grep` and the other `codemod:*`
// modules. Those resolve to this package's Node.js implementations.
//
// `codemod/host` and `codemod/testing` register these hooks themselves.

import module from "node:module";

import { CODEMOD_MODULES, resolveCodemodModule } from "./runtime/modules.mjs";

export { resolveCodemodModule };

const REGISTERED = Symbol.for("codemod.register");

const bunModules =
  typeof globalThis.Bun !== "undefined" && !globalThis[REGISTERED]
    ? await Promise.all(
        CODEMOD_MODULES.map(async (specifier) => [
          specifier,
          await import(resolveCodemodModule(specifier)),
        ]),
      )
    : [];

function register() {
  if (globalThis[REGISTERED]) return;
  globalThis[REGISTERED] = true;
  if (typeof globalThis.Bun !== "undefined") {
    // Bun's runtime plugins do not resolve imports of loaded modules through
    // onResolve, but serve virtual modules: back each with the real one.
    globalThis.Bun.plugin({
      name: "codemod",
      setup(build) {
        for (const [specifier, exports] of bunModules) {
          build.module(specifier, () => ({ exports, loader: "object" }));
        }
      },
    });
    return;
  }
  if (typeof module.registerHooks === "function") {
    module.registerHooks({
      resolve(specifier, context, nextResolve) {
        const url = resolveCodemodModule(specifier);
        return url ? { url, shortCircuit: true } : nextResolve(specifier, context);
      },
    });
    return;
  }
  if (typeof module.register === "function") {
    module.register("./runtime/hooks.mjs", import.meta.url);
    return;
  }
  throw new Error(
    "this Node.js version cannot resolve codemod:* modules; use Node.js 20.6 or later",
  );
}

register();

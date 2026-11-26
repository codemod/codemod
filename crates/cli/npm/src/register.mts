// `import "codemod/register"`: lets Node.js and Bun import code written with
// the older `codemod:workflow`, `codemod:ast-grep`, ... spelling, mapping it
// to this package's `codemod/workflow`, `codemod/ast-grep`, ... exports.
// Code that imports `codemod/*` needs none of this.
//
// `codemod/host` and `codemod/testing` register these hooks themselves.

/**
 * `import "codemod/register"` lets Node.js and Bun import code written with
 * the older `codemod:workflow`, `codemod:ast-grep`, `codemod:runtime` and
 * `codemod:llm` spelling: those imports resolve to this
 * package's `codemod/*` exports. Code that imports `codemod/*` needs none of this.
 *
 * @module
 * @experimental
 */

import module from "node:module";

import { CODEMOD_MODULES, resolveCodemodModule } from "./runtime/modules.mts";
import { isYamlUrl, loadWorkflow, loadYaml } from "./runtime/transforms.mts";

/** @experimental */
export { resolveCodemodModule };

/** What this file uses of Bun's `Bun` global. */
interface BunLike {
  plugin(plugin: {
    name: string;
    setup(build: {
      module(specifier: string, load: () => { exports: unknown; loader: "object" }): void;
    }): void;
  }): void;
}

const REGISTERED = Symbol.for("codemod.register");
const shared = globalThis as { Bun?: BunLike; [REGISTERED]?: boolean };

const bunModules: [specifier: string, exports: unknown][] =
  shared.Bun !== undefined && !shared[REGISTERED]
    ? await Promise.all(
        CODEMOD_MODULES.map(
          async (specifier): Promise<[string, unknown]> => [
            specifier,
            await import(resolveCodemodModule(specifier) ?? specifier),
          ],
        ),
      )
    : [];

function register(): void {
  if (shared[REGISTERED]) return;
  shared[REGISTERED] = true;
  if (shared.Bun !== undefined) {
    // Bun's runtime plugins do not resolve imports of loaded modules through
    // onResolve, but serve virtual modules: back each with the real one.
    shared.Bun.plugin({
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
      // The workflow file `codemod/host` imports: its imported transforms
      // become references to their files, as in the CLI's build.
      // YAML files load as their content (`import rules from "./rules.yaml"`).
      load(url, context, nextLoad) {
        if (isYamlUrl(url)) return loadYaml(url);
        return loadWorkflow(url, nextLoad(url, context));
      },
    });
    return;
  }
  if (typeof module.register === "function") {
    // The built hooks file, next to the built form of this one.
    module.register("./runtime/hooks.mjs", import.meta.url);
    return;
  }
  throw new Error(
    "this Node.js version cannot resolve codemod:* modules; use Node.js 20.6 or later",
  );
}

register();

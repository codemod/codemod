// `codemod/vite`: a Vite (and Vitest) plugin for code written with the older
// `codemod:workflow`, `codemod:ast-grep`, ... spelling: it resolves those to
// this package's `codemod/*` exports. Code that imports `codemod/*` needs no
// plugin.

/**
 * A Vite (and Vitest) plugin that resolves the older `codemod:*` spelling of
 * the codemod modules (`codemod:workflow`) to the `codemod/*` exports. Code
 * that imports `codemod/*` needs no plugin.
 *
 * ```ts
 * // vitest.config.ts
 * import { defineConfig } from "vitest/config";
 * import codemod from "codemod/vite";
 *
 * export default defineConfig({ plugins: [codemod()] });
 * ```
 *
 * @module
 */

import { fileURLToPath } from "node:url";

import { resolveCodemodModule } from "./runtime/modules.mts";

export interface CodemodVitePlugin {
  name: "codemod";
  enforce: "pre";
  resolveId(id: string): string | null;
}

export function codemod(): CodemodVitePlugin {
  return {
    name: "codemod",
    enforce: "pre",
    resolveId(id) {
      const url = resolveCodemodModule(id);
      return url ? fileURLToPath(url) : null;
    },
  };
}

export default codemod;

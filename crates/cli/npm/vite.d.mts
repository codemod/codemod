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

export interface CodemodVitePlugin {
  name: "codemod";
  enforce: "pre";
  resolveId(id: string): string | null;
}

export function codemod(): CodemodVitePlugin;
export default codemod;

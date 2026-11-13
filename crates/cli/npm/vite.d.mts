/**
 * A Vite (and Vitest) plugin that resolves the `codemod:*` modules workflows
 * import, so tests can load workflows through Vitest.
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

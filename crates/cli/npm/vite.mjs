// `codemod/vite`: a Vite (and Vitest) plugin that resolves `codemod:workflow`,
// `codemod:ast-grep` and the other `codemod:*` modules to this package's
// Node.js implementations, so tests can import workflows through Vitest,
// with its transforms and module mocks.
//
//   // vitest.config.ts
//   import { defineConfig } from "vitest/config";
//   import codemod from "codemod/vite";
//   export default defineConfig({ plugins: [codemod()] });

import { fileURLToPath } from "node:url";

import { resolveCodemodModule } from "./runtime/modules.mjs";

export function codemod() {
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

// `codemod/vite`: a Vite (and Vitest) plugin for code written with the older
// `codemod:workflow`, `codemod:ast-grep`, ... spelling: it resolves those to
// this package's `codemod/*` exports. Code that imports `codemod/*` needs no
// plugin.
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

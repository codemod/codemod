// The `codemod:*` modules and this package's Node.js implementations of them.

const MODULES = {
  "codemod:workflow": "./workflow.mjs",
  "codemod:ast-grep": "./ast-grep.mjs",
  "codemod:metrics": "./metrics.mjs",
  "codemod:runtime": "./runtime.mjs",
  "codemod:llm": "./llm.mjs",
};

export const CODEMOD_MODULES = Object.keys(MODULES);

/** The URL a `codemod:*` specifier resolves to, or undefined. */
export function resolveCodemodModule(specifier) {
  const path = MODULES[specifier];
  return path ? new URL(path, import.meta.url).href : undefined;
}

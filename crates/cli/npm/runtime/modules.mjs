// The older `codemod:*` spelling of the codemod modules, and this package's
// Node.js implementations they resolve to. `codemod/workflow` and the rest
// need no mapping: they are this package's own exports.

const MODULES = {
  "codemod:workflow": "./workflow.mjs",
  "codemod:ast-grep": "./ast-grep.mjs",
  "codemod:metrics": "./metrics.mjs",
  "codemod:runtime": "./runtime.mjs",
  "codemod:llm": "./llm.mjs",
};

export const CODEMOD_MODULES = Object.keys(MODULES);

/** The URL an older `codemod:*` specifier resolves to, or undefined. */
export function resolveCodemodModule(specifier) {
  const path = MODULES[specifier];
  return path ? new URL(path, import.meta.url).href : undefined;
}

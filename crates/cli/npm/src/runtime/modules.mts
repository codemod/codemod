// The older `codemod:*` spelling of the codemod modules, and this package's
// Node.js implementations they resolve to. `codemod/workflow` and the rest
// need no mapping: they are this package's own exports.

// Paths of the built files, next to the built form of this one.
const MODULES: Readonly<Record<string, string>> = {
  "codemod:workflow": "./workflow.mjs",
  "codemod:ast-grep": "./ast-grep.mjs",
  "codemod:runtime": "./runtime.mjs",
  "codemod:llm": "./llm.mjs",
};

export const CODEMOD_MODULES: readonly string[] = Object.keys(MODULES);

/** The URL an older `codemod:*` specifier resolves to, or undefined. */
export function resolveCodemodModule(specifier: string): string | undefined {
  const path = MODULES[specifier];
  return path ? new URL(path, import.meta.url).href : undefined;
}

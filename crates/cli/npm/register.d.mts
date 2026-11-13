/**
 * `import "codemod/register"` lets Node.js and Bun import code written with
 * the older `codemod:workflow`, `codemod:ast-grep`, `codemod:metrics`,
 * `codemod:runtime` and `codemod:llm` spelling: those imports resolve to this
 * package's `codemod/*` exports. Code that imports `codemod/*` needs none of this.
 *
 * @module
 */

/** The URL a `codemod:*` specifier resolves to, or `undefined`. */
export function resolveCodemodModule(specifier: string): string | undefined;

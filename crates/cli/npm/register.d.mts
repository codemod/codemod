/**
 * `import "codemod/register"` lets Node.js and Bun import workflows: their
 * `codemod:workflow`, `codemod:ast-grep`, `codemod:metrics`, `codemod:runtime`
 * and `codemod:llm` imports resolve to this package's Node.js implementations.
 *
 * @module
 */

/** The URL a `codemod:*` specifier resolves to, or `undefined`. */
export function resolveCodemodModule(specifier: string): string | undefined;

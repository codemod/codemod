/**
 * Benchmark transform 2: rewrite the module specifier of named imports from
 * `vitest` to a (hypothetical) renamed package, `@acme/test`.
 *
 * `vitest` is the most widely named-imported specifier in the pinned vite
 * checkout (packages/*, playground/*, and scripts). Only `import { ... } from
 * "vitest"` and `import type { ... } from "vitest"` statements are rewritten;
 * default/namespace-only imports, re-exports, and subpaths such as
 * `vitest/config` are left alone. The original quote style is preserved.
 *
 * Shared verbatim by the YAML workflow and the TypeScript workflow. Type-only
 * imports, no selector, no semantic analysis, no module-level state.
 */
import type { SgRoot } from "codemod:ast-grep";
import type TS from "codemod:ast-grep/langs/typescript";

export default function renameVitestImport(root: SgRoot<TS>): string | null {
  const from = "vitest";
  const to = "@acme/test";
  const program = root.root();
  const imports = program.findAll({
    rule: {
      kind: "import_statement",
      has: { kind: "import_clause", has: { kind: "named_imports" } },
    },
  });
  const edits = [];
  for (const statement of imports) {
    const source = statement.field("source");
    if (source === null) continue;
    const text = source.text();
    const quote = text.charAt(0);
    if (text.slice(1, -1) !== from) continue;
    edits.push(source.replace(`${quote}${to}${quote}`));
  }
  if (edits.length === 0) return null;
  return program.commitEdits(edits);
}

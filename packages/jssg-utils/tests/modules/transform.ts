import type { Edit, SgRoot } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { importFromDeclarations } from "../../src/javascript/exports/modules.ts";

// Debarrel: import (and re-export) every binding from the module that declares it.
export default function transform(root: SgRoot<TSX>): string | null {
  const edits: Edit[] = [];
  const statements = root.root().findAll({
    rule: {
      any: [
        { kind: "import_statement" },
        { kind: "export_statement", has: { field: "source", kind: "string" } },
      ],
    },
  });
  for (const statement of statements) {
    const edit = importFromDeclarations(root, statement);
    if (edit) edits.push(edit);
  }
  return edits.length ? root.root().commitEdits(edits) : null;
}

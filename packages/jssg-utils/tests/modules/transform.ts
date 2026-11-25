import type { Edit, SgRoot } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { importFromDeclarations } from "../../src/javascript/exports/modules.ts";

// Debarrel: import every binding from the module that declares it.
export default function transform(root: SgRoot<TSX>): string | null {
  const edits: Edit[] = [];
  for (const statement of root.root().findAll({ rule: { kind: "import_statement" } })) {
    const edit = importFromDeclarations(root, statement);
    if (edit) edits.push(edit);
  }
  return edits.length ? root.root().commitEdits(edits) : null;
}

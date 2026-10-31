import type { Codemod } from "codemod:ast-grep";
import type Sql from "codemod:ast-grep/langs/sql";

const codemod: Codemod<Sql> = async (root) => {
  const rootNode = root.root();
  const tables = rootNode.findAll({
    rule: {
      kind: "object_reference",
      has: { kind: "identifier", regex: "^legacy_users$", field: "name" },
      inside: { kind: "relation" },
    },
  });
  const edits = tables.flatMap((table) => {
    const name = table.field("name");
    return name ? [name.replace("users")] : [];
  });

  return edits.length > 0 ? rootNode.commitEdits(edits) : null;
};

export default codemod;

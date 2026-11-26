// Rename a variable: every edge in the references lookup from its declared
// name, and that name itself.
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { references } = symbols("javascript");

  // Find the variable declaration
  const varDecl = root.root().find({ rule: { pattern: "const oldName = $VALUE" } });
  if (!varDecl) {
    throw new Error("Expected to find 'const oldName' declaration");
  }

  const declarator = varDecl.find({ rule: { kind: "variable_declarator" } });
  const refs = references.get(declarator.field("name")) ?? [];

  const files = new Set(refs.map((edge) => edge.node.getRoot().filename()));
  if (files.size !== 1) {
    throw new Error("Expected exactly 1 file with references, got " + files.size);
  }

  // Should find 2 references (usages of 'oldName')
  if (refs.length !== 2) {
    throw new Error("Expected 2 references to 'oldName', got " + refs.length);
  }

  // Build edits to rename all references from 'oldName' to 'newName'
  let edits = [];
  for (const { node } of refs) {
    if (node.text() !== "oldName") {
      throw new Error("Expected reference text to be 'oldName', got '" + node.text() + "'");
    }
    edits.push(node.replace("newName"));
  }

  // Also rename the declaration itself
  const nameNode = declarator.field("name");
  if (!nameNode) {
    throw new Error("Expected to find name field");
  }
  if (nameNode.text() !== "oldName") {
    throw new Error("Expected declaration name to be 'oldName', got '" + nameNode.text() + "'");
  }
  edits.push(nameNode.replace("newName"));

  if (edits.length !== 3) {
    throw new Error("Expected 3 edits (2 references + 1 declaration), got " + edits.length);
  }

  return root.root().commitEdits(edits);
}

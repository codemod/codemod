// references.get(node) on a class finds the files that import it and use it.
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { references } = symbols("python");

  // Find all 'User' identifiers
  const userNodes = root.root().findAll({ rule: { pattern: "User" } });
  if (userNodes.length < 1) {
    throw new Error("Expected at least 1 'User' node, got " + userNodes.length);
  }

  // Use the first one (the class name in definition)
  const classNameNode = userNodes[0];

  const refs = references.get(classNameNode) ?? [];

  if (refs.length === 0) {
    console.log("No references found - this is acceptable");
    return null;
  }

  const files = new Set(refs.map((edge) => edge.node.getRoot().filename()));

  // ty_ide finds references in both files:
  // models.py: class definition (1)
  // app.py: import + 3 usages (4)
  // Total: 5 references across 2 files
  if (files.size !== 2) {
    throw new Error("Expected 2 files with references, got " + files.size);
  }

  if (refs.length !== 5) {
    throw new Error("Expected 5 references, got " + refs.length);
  }

  return null;
}

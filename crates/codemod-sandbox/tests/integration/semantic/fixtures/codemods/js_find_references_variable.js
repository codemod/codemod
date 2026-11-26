// references.get(node) on a declared variable's name gives one edge per use
// of the variable, not the declaration itself. (The whole `const` statement
// has no edges: ask from the name.)
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { references } = symbols("javascript");

  // Find the variable declaration 'counter'
  const varDecl = root.root().find({ rule: { pattern: "const counter = $VALUE" } });
  if (!varDecl) {
    throw new Error("Expected to find 'const counter' declaration");
  }

  const name = varDecl.find({ rule: { kind: "variable_declarator" } }).field("name");
  const refs = references.get(name) ?? [];

  // All in this file
  const files = new Set(refs.map((edge) => edge.node.getRoot().filename()));
  if (files.size !== 1) {
    throw new Error("Expected exactly 1 file with references, got " + files.size);
  }

  // Should find 3 references (usages of 'counter', not the definition)
  if (refs.length !== 3) {
    throw new Error("Expected 3 references to 'counter', got " + refs.length);
  }

  // All references should be 'counter'
  for (const { node } of refs) {
    if (node.text() !== "counter") {
      throw new Error("Expected reference text to be 'counter', got '" + node.text() + "'");
    }
  }

  return null;
}

// references.get(node) in file scope, on a declared variable's name: one edge
// per use, all in this file. (The whole `const` statement has no edges.)
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { references } = symbols("typescript");

  // Find 'const x = 1'
  const node = root.root().find({ rule: { pattern: "const x = 1" } });
  if (!node) {
    throw new Error("Expected to find 'const x = 1'");
  }

  const name = node.find({ rule: { kind: "variable_declarator" } }).field("name");
  const refs = references.get(name) ?? [];

  const files = new Set(refs.map((edge) => edge.node.getRoot().filename()));
  if (files.size !== 1) {
    throw new Error("Expected exactly 1 file with references, got " + files.size);
  }

  // Should find 2 references (x + 2 and console.log(x))
  if (refs.length !== 2) {
    throw new Error("Expected 2 references to 'x', got " + refs.length);
  }

  // Check that nodes have expected methods and values
  for (const { node } of refs) {
    if (typeof node.text !== "function") {
      throw new Error("Expected node.text to be a function");
    }
    if (node.text() !== "x") {
      throw new Error("Expected reference text to be 'x', got '" + node.text() + "'");
    }
    // Check that the node's root has expected methods
    if (typeof node.getRoot().filename !== "function") {
      throw new Error("Expected root.filename to be a function");
    }
  }

  return null;
}

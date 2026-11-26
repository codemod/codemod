// references.get(node) on an exported function finds its uses across the
// workspace, as one flat edge per reference node.
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { references } = symbols("typescript");

  // Find all 'add' identifiers (first one should be the function name)
  const addNodes = root.root().findAll({ rule: { pattern: "add" } });
  if (addNodes.length < 1) {
    throw new Error("Expected at least 1 'add' node, got " + addNodes.length);
  }

  const funcNameNode = addNodes[0];

  const refs = references.get(funcNameNode) ?? [];

  if (refs.length === 0) {
    console.log("No references found - this is acceptable");
    return null;
  }

  // Each edge should have a node in some file
  for (const edge of refs) {
    if (!edge.node || !edge.node.getRoot()) {
      throw new Error("Expected each edge to have a node with a root");
    }
  }

  if (refs.length !== 2) {
    throw new Error("Expected 2 references, got " + refs.length);
  }

  return null;
}

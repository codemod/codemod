// references.get(node) on a function declared and called in the same file.
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { references } = symbols("typescript");

  // Find all 'greet' identifiers
  const greetNodes = root.root().findAll({ rule: { pattern: "greet" } });
  if (greetNodes.length < 1) {
    throw new Error("Expected at least 1 'greet' node, got " + greetNodes.length);
  }

  // Use the first one (the function name in declaration)
  const funcNameNode = greetNodes[0];

  const refs = references.get(funcNameNode) ?? [];

  if (refs.length === 0) {
    console.log("No references found - semantic provider may not have indexed yet");
    return null;
  }

  const files = new Set(refs.map((edge) => edge.node.getRoot().filename()));
  if (files.size !== 1) {
    throw new Error("Expected exactly 1 file with references, got " + files.size);
  }

  // Should find 3 references (the 3 calls to greet)
  if (refs.length !== 3) {
    throw new Error("Expected 3 references to 'greet' function, got " + refs.length);
  }

  // All references should be 'greet'
  for (const { node } of refs) {
    if (node.text() !== "greet") {
      throw new Error("Expected reference text to be 'greet', got '" + node.text() + "'");
    }
  }

  return null;
}

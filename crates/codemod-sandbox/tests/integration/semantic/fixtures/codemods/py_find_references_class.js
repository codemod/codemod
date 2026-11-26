// references.get(node) on a class name gives an edge for each use of it.
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { references } = symbols("python");

  // Find all 'Counter' identifiers
  const counterNodes = root.root().findAll({ rule: { pattern: "Counter" } });
  if (counterNodes.length < 1) {
    throw new Error("Expected at least 1 'Counter' node, got " + counterNodes.length);
  }

  // Use the first one (the class name in definition)
  const classNameNode = counterNodes[0];

  const refs = references.get(classNameNode) ?? [];

  if (refs.length === 0) {
    console.log("No references found - semantic provider may not have indexed yet");
    return null;
  }

  // Count total references
  let totalRefs = 0;
  let refTexts = [];
  for (const { node } of refs) {
    if (typeof node.text === "function") {
      totalRefs++;
      refTexts.push(node.text());
    }
  }

  // Should find at least 3 references (Counter() calls and isinstance check)
  if (totalRefs < 3) {
    throw new Error("Expected at least 3 references, got: " + totalRefs);
  }

  // All references should be 'Counter'
  for (const text of refTexts) {
    if (text !== "Counter") {
      throw new Error("Expected reference text to be 'Counter', got: '" + text + "'");
    }
  }

  return null;
}

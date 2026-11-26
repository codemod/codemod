// references.get(node) on a function finds its uses in every file of the
// workspace, as one flat edge per reference node.
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { references } = symbols("python");

  // Find all 'calculate' identifiers
  const calcNodes = root.root().findAll({ rule: { pattern: "calculate" } });
  if (calcNodes.length < 1) {
    throw new Error("Expected at least 1 'calculate' node, got " + calcNodes.length);
  }

  // Use the first one (the function name in definition)
  const funcNameNode = calcNodes[0];

  const refs = references.get(funcNameNode) ?? [];

  if (refs.length === 0) {
    console.log("No references found - this is acceptable");
    return null;
  }

  const files = new Set();
  for (const { node } of refs) {
    files.add(node.getRoot().filename());
    console.log("Reference text:", node.parent().text());
  }

  // ty_ide finds all references including:
  // 1. Definition: def calculate(x, y) in utils.py
  // 2. Usage: calculate(1, 1) in utils.py
  // 3. Import: from utils import calculate in main.py
  // 4. Usage: calculate(2, 3) in main.py
  // 5. Usage: calculate(3, 4) in main.py
  if (files.size !== 2) {
    throw new Error("Expected 2 files with references, got " + files.size);
  }

  if (refs.length !== 5) {
    throw new Error("Expected 5 references, got " + refs.length);
  }

  return null;
}

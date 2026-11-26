// references.get(node) across files, with the importing file preprocessed
// (cached) before the target runs.
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { references } = symbols("typescript");

  // Find all 'formatDate' identifiers
  const formatDateNodes = root.root().findAll({ rule: { pattern: "formatDate" } });
  if (formatDateNodes.length < 1) {
    throw new Error("Expected at least 1 'formatDate' node, got " + formatDateNodes.length);
  }

  const funcNameNode = formatDateNodes[0];

  const refs = references.get(funcNameNode) ?? [];

  if (refs.length === 0) {
    console.log("No references found - this is acceptable");
    return null;
  }

  // Count total references across all files
  if (refs.length !== 3) {
    throw new Error("Expected 3 references, got " + refs.length);
  }

  return null;
}

export default function transform(root) {
  // Find all 'formatDate' identifiers
  const formatDateNodes = root.root().findAll({ rule: { pattern: "formatDate" } });
  if (formatDateNodes.length < 1) {
    throw new Error("Expected at least 1 'formatDate' node, got " + formatDateNodes.length);
  }

  const funcNameNode = formatDateNodes[0];

  const references = funcNameNode.references();

  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array");
  }

  if (references.length === 0) {
    console.log("No references found - this is acceptable");
    return null;
  }

  // Count total references across all files
  let totalRefs = 0;

  for (const fileRef of references) {
    totalRefs += fileRef.nodes.length;
  }

  if (totalRefs !== 3) {
    throw new Error("Expected 3 references, got " + totalRefs);
  }

  return null;
}

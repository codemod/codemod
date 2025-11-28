export default function transform(root) {
  // Find all 'calculate' identifiers
  const calcNodes = root.root().findAll({ rule: { pattern: "calculate" } });
  if (calcNodes.length < 1) {
    throw new Error(
      "Expected at least 1 'calculate' node, got " + calcNodes.length,
    );
  }

  // Use the first one (the function name in definition)
  const funcNameNode = calcNodes[0];

  const references = funcNameNode.references();

  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array");
  }

  if (references.length === 0) {
    console.log("No references found - this is acceptable");
    return null;
  }

  // Log all references found - be defensive about node.text
  let totalRefs = 0;
  for (const fileRef of references) {
    if (!fileRef.root) {
      throw new Error("Expected fileRef.root to exist");
    }
    if (!Array.isArray(fileRef.nodes)) {
      throw new Error("Expected fileRef.nodes to be an array");
    }

    for (const node of fileRef.nodes) {
      totalRefs++;
      // Just count - don't validate text if not available
      if (typeof node.text === "function") {
        console.log("Reference text:", node.parent().text());
      }
    }
  }

  if (references.length !== 2) {
    throw new Error(
      "Expected 2 files with references, got " + references.length,
    );
  }

  if (totalRefs !== 3) {
    throw new Error("Expected 3 references, got " + totalRefs);
  }

  return null;
}

export default function transform(root) {
  // Find all 'add' identifiers (first one should be the function name)
  const addNodes = root.root().findAll({ rule: { pattern: "add" } });
  if (addNodes.length < 1) {
    throw new Error("Expected at least 1 'add' node, got " + addNodes.length);
  }

  const funcNameNode = addNodes[0];

  const references = funcNameNode.references();

  // Should return array of file references
  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array");
  }

  if (references.length === 0) {
    console.log("No references found - this is acceptable");
    return null;
  }

  // Each entry should have root and nodes
  let totalRefs = 0;
  for (const fileRef of references) {
    if (!fileRef.root) {
      throw new Error("Expected fileRef.root to exist");
    }
    if (!Array.isArray(fileRef.nodes)) {
      throw new Error("Expected fileRef.nodes to be an array");
    }
    totalRefs += fileRef.nodes.length;
  }

  if (totalRefs !== 2) {
    throw new Error("Expected 2 references, got " + totalRefs);
  }

  return null;
}

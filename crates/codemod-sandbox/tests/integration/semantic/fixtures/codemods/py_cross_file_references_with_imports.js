export default function transform(root) {
  // Find all 'User' identifiers
  const userNodes = root.root().findAll({ rule: { pattern: "User" } });
  if (userNodes.length < 1) {
    throw new Error("Expected at least 1 'User' node, got " + userNodes.length);
  }

  // Use the first one (the class name in definition)
  const classNameNode = userNodes[0];

  const references = classNameNode.references();

  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array");
  }

  if (references.length === 0) {
    console.log("No references found - this is acceptable");
    return null;
  }

  // Count references across all files - be defensive
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

  if (references.length !== 1) {
    throw new Error(
      "Expected 1 file with references, got " + references.length,
    );
  }

  if (totalRefs !== 3) {
    throw new Error("Expected 3 references, got " + totalRefs);
  }

  return null;
}

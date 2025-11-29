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

  // ty_ide finds references in both files:
  // models.py: class definition (1)
  // app.py: import + 3 usages (4)
  // Total: 5 references across 2 files
  if (references.length !== 2) {
    throw new Error(
      "Expected 2 files with references, got " + references.length,
    );
  }

  if (totalRefs !== 5) {
    throw new Error("Expected 5 references, got " + totalRefs);
  }

  return null;
}

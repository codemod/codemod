export default function transform(root) {
  // Find all 'Counter' identifiers
  const counterNodes = root.root().findAll({ rule: { pattern: "Counter" } });
  if (counterNodes.length < 1) {
    throw new Error(
      "Expected at least 1 'Counter' node, got " + counterNodes.length,
    );
  }

  // Use the first one (the class name in definition)
  const classNameNode = counterNodes[0];

  const references = classNameNode.references();

  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array");
  }

  if (references.length === 0) {
    console.log(
      "No references found - semantic provider may not have indexed yet",
    );
    return null;
  }

  // Count total references
  let totalRefs = 0;
  let refTexts = [];
  for (const fileRef of references) {
    for (const node of fileRef.nodes) {
      if (typeof node.text === "function") {
        totalRefs++;
        refTexts.push(node.text());
      }
    }
  }

  // Should find at least 3 references (Counter() calls and isinstance check)
  if (totalRefs < 3) {
    throw new Error("Expected at least 3 references, got: " + totalRefs);
  }

  // All references should be 'Counter'
  for (const text of refTexts) {
    if (text !== "Counter") {
      throw new Error(
        "Expected reference text to be 'Counter', got: '" + text + "'",
      );
    }
  }

  return null;
}

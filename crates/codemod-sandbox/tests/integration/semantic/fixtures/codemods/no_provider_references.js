export default function transform(root) {
  const node = root.root().find({ rule: { pattern: "x" } });
  if (!node) {
    throw new Error("Expected to find 'x' node");
  }

  const references = node.references();

  // Should return empty array when no provider is configured
  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array, got: " + typeof references);
  }

  if (references.length !== 0) {
    throw new Error(
      "Expected empty array when no semantic provider is configured, got length: " +
        references.length,
    );
  }

  return null;
}

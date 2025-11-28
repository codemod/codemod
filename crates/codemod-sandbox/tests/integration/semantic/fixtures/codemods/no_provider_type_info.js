export default function transform(root) {
  const node = root.root().find({ rule: { pattern: "x" } });
  if (!node) {
    throw new Error("Expected to find 'x' node");
  }

  const typeInfo = node.typeInfo();

  // Should return null when no provider is configured
  if (typeInfo !== null) {
    throw new Error(
      "Expected null when no semantic provider is configured, got: " + typeInfo,
    );
  }

  return null;
}

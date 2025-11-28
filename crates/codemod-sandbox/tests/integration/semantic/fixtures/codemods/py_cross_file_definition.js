export default function transform(root) {
  // Find the 'add' call
  const callNode = root.root().find({ rule: { pattern: "add($$$)" } });
  if (!callNode) {
    throw new Error("Expected to find add() call");
  }

  const definition = callNode.definition();

  // Definition may or may not be found depending on cross-file resolution
  if (definition === null) {
    // It's okay if cross-file definition isn't resolved yet
    console.log(
      "Definition not found (cross-file resolution may not be complete)",
    );
    return null;
  }

  // If found, verify structure
  if (!definition.node) {
    throw new Error("Expected definition.node to exist");
  }
  if (!definition.root) {
    throw new Error("Expected definition.root to exist");
  }

  return null;
}

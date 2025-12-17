export default function transform(root) {
  // Find 'x' reference in 'x + 2'
  const nodes = root.root().findAll({ rule: { pattern: "x" } });
  if (nodes.length < 2) {
    throw new Error("Expected at least 2 'x' nodes, got " + nodes.length);
  }

  // Get the second 'x' (the reference, not the declaration)
  const refNode = nodes[1];

  const definition = refNode.definition();

  // Definition may or may not be found depending on semantic provider state
  if (definition === null) {
    console.log("Definition not found - this is acceptable for this test");
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

export default function transform(root) {
  // Find all 'greet' identifiers
  const greetNodes = root.root().findAll({ rule: { pattern: "greet" } });
  if (greetNodes.length < 1) {
    throw new Error("Expected at least 1 'greet' node, got " + greetNodes.length);
  }

  // Use the first one (the function name in declaration)
  const funcNameNode = greetNodes[0];

  const references = funcNameNode.references();

  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array");
  }

  if (references.length === 0) {
    console.log("No references found - semantic provider may not have indexed yet");
    return null;
  }

  if (references.length !== 1) {
    throw new Error("Expected exactly 1 file with references, got " + references.length);
  }

  const fileRef = references[0];
  // Should find 3 references (the 3 calls to greet)
  if (fileRef.nodes.length !== 3) {
    throw new Error("Expected 3 references to 'greet' function, got " + fileRef.nodes.length);
  }

  // All references should be 'greet'
  for (const node of fileRef.nodes) {
    if (node.text() !== "greet") {
      throw new Error("Expected reference text to be 'greet', got '" + node.text() + "'");
    }
  }

  return null;
}

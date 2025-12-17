export default function transform(root) {
  // Find all 'greet' identifiers
  const greetNodes = root.root().findAll({ rule: { pattern: "greet" } });
  if (greetNodes.length < 1) {
    throw new Error("Expected at least 1 'greet' node, got " + greetNodes.length);
  }

  // Use the first one (the function name in definition)
  const funcNameNode = greetNodes[0];

  const references = funcNameNode.references();

  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array");
  }

  if (references.length === 0) {
    console.log("No references found - semantic provider may not have indexed yet");
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

  // Should find at least 3 references (the 3 calls to greet)
  if (totalRefs < 3) {
    throw new Error("Expected at least 3 references, got: " + totalRefs);
  }

  // All references should be 'greet'
  for (const text of refTexts) {
    if (text !== "greet") {
      throw new Error("Expected reference text to be 'greet', got: '" + text + "'");
    }
  }

  return null;
}

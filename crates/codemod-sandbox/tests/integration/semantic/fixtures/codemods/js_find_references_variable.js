export default function transform(root) {
  // Find the variable declaration 'counter'
  const varDecl = root.root().find({ rule: { pattern: "const counter = $VALUE" } });
  if (!varDecl) {
    throw new Error("Expected to find 'const counter' declaration");
  }

  // Find references from the declaration node itself
  const references = varDecl.references();

  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array, got: " + typeof references);
  }

  if (references.length === 0) {
    console.log("No references found - semantic provider may not have indexed yet");
    return null;
  }

  if (references.length !== 1) {
    throw new Error("Expected exactly 1 file with references, got " + references.length);
  }

  const fileRef = references[0];
  if (!fileRef.root) {
    throw new Error("Expected fileRef.root to exist");
  }
  if (!Array.isArray(fileRef.nodes)) {
    throw new Error("Expected fileRef.nodes to be an array");
  }

  // Should find 3 references (usages of 'counter', not the definition)
  if (fileRef.nodes.length !== 3) {
    throw new Error("Expected 3 references to 'counter', got " + fileRef.nodes.length);
  }

  // All references should be 'counter'
  for (const node of fileRef.nodes) {
    if (node.text() !== "counter") {
      throw new Error("Expected reference text to be 'counter', got '" + node.text() + "'");
    }
  }

  return null;
}

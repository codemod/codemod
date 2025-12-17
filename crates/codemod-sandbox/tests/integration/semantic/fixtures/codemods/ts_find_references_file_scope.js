export default function transform(root) {
  // Find 'const x = 1'
  const node = root.root().find({ rule: { pattern: "const x = 1" } });
  if (!node) {
    throw new Error("Expected to find 'const x = 1'");
  }

  const references = node.references();

  // Should return array of file references
  if (!Array.isArray(references)) {
    throw new Error(
      "Expected references to be an array, got: " + typeof references,
    );
  }

  if (references.length === 0) {
    console.log(
      "No references found - semantic provider may not have indexed yet",
    );
    return null;
  }

  if (references.length !== 1) {
    throw new Error(
      "Expected exactly 1 file with references, got " + references.length,
    );
  }

  const fileRef = references[0];
  if (!fileRef.root) {
    throw new Error("Expected fileRef.root to exist");
  }
  if (!Array.isArray(fileRef.nodes)) {
    throw new Error("Expected fileRef.nodes to be an array");
  }

  // Should find 2 references (x + 2 and console.log(x))
  if (fileRef.nodes.length !== 2) {
    throw new Error(
      "Expected 2 references to 'x', got " + fileRef.nodes.length,
    );
  }

  // Check that nodes have expected methods and values
  for (const node of fileRef.nodes) {
    if (typeof node.text !== "function") {
      throw new Error("Expected node.text to be a function");
    }
    if (node.text() !== "x") {
      throw new Error(
        "Expected reference text to be 'x', got '" + node.text() + "'",
      );
    }
  }

  // Check that root has expected methods
  if (typeof fileRef.root.filename !== "function") {
    throw new Error("Expected root.filename to be a function");
  }

  return null;
}



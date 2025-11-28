export default function transform(root) {
  // Find the first assignment 'counter = 0'
  const assignment = root.root().find({ rule: { pattern: "counter = 0" } });
  if (!assignment) {
    throw new Error("Expected to find 'counter = 0'");
  }

  const references = assignment.references();

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

  // Collect reference texts
  let refTexts = [];
  for (const fileRef of references) {
    if (!fileRef.root) {
      throw new Error("Expected fileRef.root to exist");
    }
    if (!Array.isArray(fileRef.nodes)) {
      throw new Error("Expected fileRef.nodes to be an array");
    }

    for (const node of fileRef.nodes) {
      if (typeof node.text === "function") {
        refTexts.push(node.text());
      }
    }
  }

  // Should find references to 'counter'
  if (refTexts.length === 0) {
    throw new Error(
      "Expected to find references to 'counter', got " + refTexts.length,
    );
  }

  // All references should be 'counter'
  for (const text of refTexts) {
    if (text !== "counter") {
      throw new Error(
        "Expected reference text to be 'counter', got: '" + text + "'",
      );
    }
  }

  return null;
}

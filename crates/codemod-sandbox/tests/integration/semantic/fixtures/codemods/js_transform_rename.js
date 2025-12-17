export default function transform(root) {
  // Find the variable declaration
  const varDecl = root.root().find({ rule: { pattern: "const oldName = $VALUE" } });
  if (!varDecl) {
    throw new Error("Expected to find 'const oldName' declaration");
  }

  const references = varDecl.references();

  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array");
  }

  if (references.length === 0) {
    console.log("No references found - cannot perform rename");
    return null;
  }

  if (references.length !== 1) {
    throw new Error("Expected exactly 1 file with references, got " + references.length);
  }

  const fileRef = references[0];
  // Should find 2 references (usages of 'oldName')
  if (fileRef.nodes.length !== 2) {
    throw new Error("Expected 2 references to 'oldName', got " + fileRef.nodes.length);
  }

  // Build edits to rename all references from 'oldName' to 'newName'
  let edits = [];
  for (const node of fileRef.nodes) {
    if (node.text() !== "oldName") {
      throw new Error("Expected reference text to be 'oldName', got '" + node.text() + "'");
    }
    edits.push(node.replace("newName"));
  }

  // Also rename the declaration itself
  const nameNode = varDecl.field("name");
  if (!nameNode) {
    throw new Error("Expected to find name field");
  }
  if (nameNode.text() !== "oldName") {
    throw new Error("Expected declaration name to be 'oldName', got '" + nameNode.text() + "'");
  }
  edits.push(nameNode.replace("newName"));

  if (edits.length !== 3) {
    throw new Error("Expected 3 edits (2 references + 1 declaration), got " + edits.length);
  }

  return root.root().commitEdits(edits);
}

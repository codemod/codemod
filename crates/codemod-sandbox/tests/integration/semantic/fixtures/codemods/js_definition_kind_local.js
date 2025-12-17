export default function transform(root) {
  // Find the reference to myVar in console.log
  const ref = root.root().find({ rule: { pattern: "console.log(myVar)" } });
  if (!ref) {
    throw new Error("Expected to find 'console.log(myVar)'");
  }

  // Find the myVar identifier inside the call
  const myVarRef = ref.find({ rule: { pattern: "myVar" } });
  if (!myVarRef) {
    throw new Error("Expected to find 'myVar' reference");
  }

  // Get definition
  const def = myVarRef.definition();

  if (!def) {
    throw new Error("Expected to find definition for 'myVar'");
  }

  // Check the kind field
  if (def.kind !== "local") {
    throw new Error(
      "Expected definition kind to be 'local', got '" + def.kind + "'",
    );
  }

  // Verify it's pointing to the right node
  if (!def.node.text().includes("myVar")) {
    throw new Error(
      "Expected definition node to contain 'myVar', got '" +
        def.node.text() +
        "'",
    );
  }

  console.log("Definition kind:", def.kind);

  return null;
}



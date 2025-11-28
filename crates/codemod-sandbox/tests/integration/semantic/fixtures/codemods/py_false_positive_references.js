export default function transform(root) {
  const filename = root.filename();

  // Find all 'my_var' identifiers
  const myVarNodes = root.root().findAll({ rule: { pattern: "my_var" } });
  if (myVarNodes.length < 1) {
    throw new Error(
      "Expected at least 1 'my_var' node, got " + myVarNodes.length,
    );
  }

  // Use the first one (the variable definition)
  const myVarNode = myVarNodes[0];

  const references = myVarNode.references();

  if (!Array.isArray(references)) {
    throw new Error("Expected references to be an array");
  }

  // Test different expectations based on which file we're in
  if (filename.endsWith("app.py")) {
    // app.py should have 2 file references:
    // 1. app.py itself (with the print statement using my_var)
    // 2. really_imports.py (which imports from app)
    if (references.length !== 2) {
      throw new Error(
        "app.py: Expected 2 file references, got " + references.length,
      );
    }

    // Verify one reference is in app.py
    const appRef = references.find((r) => r.root.filename().endsWith("app.py"));
    if (!appRef) {
      throw new Error("app.py: Expected a reference in app.py itself");
    }

    // Verify one reference is in really_imports.py
    const importRef = references.find((r) =>
      r.root.filename().endsWith("really_imports.py"),
    );
    if (!importRef) {
      throw new Error("app.py: Expected a reference in really_imports.py");
    }
  } else if (filename.endsWith("models.py")) {
    // models.py should only have 1 file reference (itself)
    // because really_imports.py imports from app, NOT from models
    if (references.length !== 1) {
      throw new Error(
        "models.py: Expected 1 file reference (no false positives from really_imports.py), got " +
          references.length,
      );
    }

    // Verify the reference is in models.py itself
    if (!references[0].root.filename().endsWith("models.py")) {
      throw new Error(
        "models.py: Expected reference to be in models.py, got " +
          references[0].root.filename(),
      );
    }
  } else if (filename.endsWith("really_imports.py")) {
    // really_imports.py defines my_var_alias (via import), not my_var
    // So references to my_var in this file should only find 1 file reference
    if (references.length !== 1) {
      throw new Error(
        "really_imports.py: Expected 1 file reference, got " +
          references.length,
      );
    }
  }

  return null;
}

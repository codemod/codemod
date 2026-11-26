// references.get(node) only reaches files that import the name from the
// module that declares it: really_imports.py imports my_var from app, so it
// is among app.py's references and not among models.py's.
import { symbols } from "codemod:ast-grep";

// The files the edges land in.
function filesOf(edges) {
  return [...new Set(edges.map((edge) => edge.node.getRoot().filename()))];
}

export default function transform(root) {
  const { references } = symbols("python");
  const filename = root.filename();

  // Find all 'my_var' identifiers
  const myVarNodes = root.root().findAll({ rule: { pattern: "my_var" } });
  if (myVarNodes.length < 1) {
    throw new Error("Expected at least 1 'my_var' node, got " + myVarNodes.length);
  }

  // Use the first one (the variable definition)
  const myVarNode = myVarNodes[0];

  const files = filesOf(references.get(myVarNode) ?? []);

  // Test different expectations based on which file we're in
  if (filename.endsWith("app.py")) {
    // app.py should have references in 2 files:
    // 1. app.py itself (with the print statement using my_var)
    // 2. really_imports.py (which imports from app)
    if (files.length !== 2) {
      throw new Error("app.py: Expected references in 2 files, got " + files.length);
    }

    // Verify one reference is in app.py
    if (!files.some((f) => f.endsWith("app.py"))) {
      throw new Error("app.py: Expected a reference in app.py itself");
    }

    // Verify one reference is in really_imports.py
    if (!files.some((f) => f.endsWith("really_imports.py"))) {
      throw new Error("app.py: Expected a reference in really_imports.py");
    }
  } else if (filename.endsWith("models.py")) {
    // models.py should only have references in 1 file (itself)
    // because really_imports.py imports from app, NOT from models
    if (files.length !== 1) {
      throw new Error(
        "models.py: Expected references in 1 file (none from really_imports.py), got " +
          files.length,
      );
    }

    // Verify the reference is in models.py itself
    if (!files[0].endsWith("models.py")) {
      throw new Error("models.py: Expected reference to be in models.py, got " + files[0]);
    }
  } else if (filename.endsWith("really_imports.py")) {
    // really_imports.py defines my_var_alias (via import), not my_var
    // So references to my_var in this file should only be in 1 file
    if (files.length !== 1) {
      throw new Error("really_imports.py: Expected references in 1 file, got " + files.length);
    }
  }

  return null;
}

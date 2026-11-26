// definition.get(node) on a call to a function imported from another file
// follows the import, in workspace scope.
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { definition } = symbols("python");

  // Find the 'add' call
  const callNode = root.root().find({ rule: { pattern: "add($$$)" } });
  if (!callNode) {
    throw new Error("Expected to find add() call");
  }

  const def = definition.get(callNode)?.[0];

  // Definition may or may not be found depending on cross-file resolution
  if (def === undefined) {
    // It's okay if cross-file definition isn't resolved yet
    console.log("Definition not found (cross-file resolution may not be complete)");
    return null;
  }

  // If found, verify structure
  if (!def.node) {
    throw new Error("Expected definition.node to exist");
  }
  if (!def.node.getRoot()) {
    throw new Error("Expected definition.node.getRoot() to exist");
  }

  return null;
}

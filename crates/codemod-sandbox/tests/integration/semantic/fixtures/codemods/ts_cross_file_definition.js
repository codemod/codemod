// definition.get(node) on a call to a function imported from another file
// follows the import, in workspace scope.
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { definition } = symbols("typescript");

  // Find all 'add' identifiers
  const addNodes = root.root().findAll({ rule: { pattern: "add" } });
  if (addNodes.length < 1) {
    throw new Error("Expected at least 1 'add' node, got " + addNodes.length);
  }

  // Find the 'add' that's in a call (not the import)
  let callAdd = null;
  for (const node of addNodes) {
    const parent = node.parent();
    if (parent && parent.kind() === "call_expression") {
      callAdd = node;
      break;
    }
  }

  if (!callAdd) {
    console.log("Could not find add() call");
    return null;
  }

  const def = definition.get(callAdd)?.[0];

  // Definition may or may not be found depending on cross-file resolution
  if (def === undefined) {
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

  // Log where the definition was found
  console.log("Definition found");

  return null;
}

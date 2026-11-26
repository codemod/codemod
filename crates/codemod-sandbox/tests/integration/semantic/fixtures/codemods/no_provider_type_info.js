// Without a semantic provider the typeOf lookup has no type for a node:
// get() answers undefined.
import { types } from "codemod:ast-grep";

export default function transform(root) {
  const node = root.root().find({ rule: { pattern: "x" } });
  if (!node) {
    throw new Error("Expected to find 'x' node");
  }

  const type = types("javascript").typeOf.get(node);
  if (type !== undefined) {
    throw new Error("Expected undefined when no semantic provider is configured, got: " + type);
  }

  return null;
}

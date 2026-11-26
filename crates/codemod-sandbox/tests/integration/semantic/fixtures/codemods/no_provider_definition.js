// Without a semantic provider, the definition lookup has no edges:
// definition.get(node) is undefined.
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { definition } = symbols("javascript");
  const node = root.root().find({ rule: { pattern: "x" } });
  if (!node) {
    throw new Error("Expected to find 'x' node");
  }

  const edges = definition.get(node);

  if (edges !== undefined) {
    throw new Error(
      "Expected undefined when no semantic provider is configured, got: " + JSON.stringify(edges),
    );
  }

  return null;
}

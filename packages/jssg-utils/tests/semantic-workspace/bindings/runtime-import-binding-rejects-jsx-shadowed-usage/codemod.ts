import { ok as assert } from "assert";
import { isRuntimeImportBinding } from "../../../../src/javascript/exports/bindings.ts";
import { requireNode, type SemanticCodemodRoot } from "../_shared.ts";

export default function transform(root: SemanticCodemodRoot) {
  const usage = root.root().find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: { kind: "jsx_self_closing_element" },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find shadowed JSX Grid usage");
  assert(
    !isRuntimeImportBinding(resolvedUsage),
    "Shadowed JSX tag identifier should not resolve to the runtime import binding",
  );
  return null;
}

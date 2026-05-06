import { ok as assert } from "assert";
import { isRuntimeImportBinding } from "../../../../src/javascript/exports/bindings.ts";
import { requireNode, type SemanticCodemodRoot } from "../_shared.ts";

export default function transform(root: SemanticCodemodRoot) {
  const usage = root.root().find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: { kind: "assignment_pattern" },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find default-parameter initializer usage");
  assert(
    isRuntimeImportBinding(resolvedUsage),
    "Default-parameter initializer usage should still resolve as the imported runtime binding",
  );
  return null;
}

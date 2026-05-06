import { ok as assert } from "assert";
import { isRuntimeImportBinding } from "../../../../src/javascript/exports/bindings.ts";
import { requireNode, type SemanticCodemodRoot } from "../_shared.ts";

export default function transform(root: SemanticCodemodRoot) {
  const usage = root.root().find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: { kind: "return_statement" },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find destructured parameter usage");
  assert(
    !isRuntimeImportBinding(resolvedUsage),
    "Destructured parameter shadow should resolve to Grid",
  );
  assert(
    !isRuntimeImportBinding(resolvedUsage),
    "Destructured parameter should shadow the imported binding",
  );
  return null;
}

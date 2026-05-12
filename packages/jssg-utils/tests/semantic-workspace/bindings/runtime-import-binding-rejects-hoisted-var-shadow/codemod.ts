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

  const resolvedUsage = requireNode(usage, "Should find hoisted var usage");
  assert(!isRuntimeImportBinding(resolvedUsage), "Hoisted var shadow should resolve to Grid");
  assert(!isRuntimeImportBinding(resolvedUsage), "Hoisted var should shadow the imported binding");
  return null;
}

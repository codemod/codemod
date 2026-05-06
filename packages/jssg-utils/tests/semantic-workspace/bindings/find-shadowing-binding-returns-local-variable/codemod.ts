import { ok as assert } from "assert";
import { findShadowingBinding } from "../../../../src/javascript/exports/bindings.ts";
import { requireNode, type SemanticCodemodRoot } from "../_shared.ts";

export default function transform(root: SemanticCodemodRoot) {
  const usage = root.root().find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: { kind: "return_statement" },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find local usage");
  const shadow = findShadowingBinding(resolvedUsage);
  const resolvedShadow = requireNode(shadow, "Should find local shadowing binding");
  assert(resolvedShadow.text() === "Grid", "Shadowing binding should be the local identifier");
  return null;
}

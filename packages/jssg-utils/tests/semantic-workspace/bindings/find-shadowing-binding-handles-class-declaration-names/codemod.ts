import { ok as assert } from "assert";
import {
  findShadowingBinding,
  isRuntimeImportBinding,
} from "../../../../src/javascript/exports/bindings.ts";
import { requireNode, type SemanticCodemodRoot } from "../_shared.ts";

export default function transform(root: SemanticCodemodRoot) {
  const usage = root.root().find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: { kind: "return_statement" },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find class declaration usage");
  const shadow = findShadowingBinding(resolvedUsage);
  const resolvedShadow = requireNode(shadow, "Should treat class declaration name as shadowing");
  assert(resolvedShadow.text() === "Grid", "Class declaration shadow should resolve to Grid");
  assert(
    !isRuntimeImportBinding(resolvedUsage),
    "Class declaration should shadow the imported binding",
  );
  return null;
}

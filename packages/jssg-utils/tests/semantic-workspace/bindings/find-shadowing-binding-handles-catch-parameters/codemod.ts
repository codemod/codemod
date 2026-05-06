import { ok as assert } from "assert";
import {
  findShadowingBinding,
  isRuntimeImportBinding,
} from "../../../../src/javascript/exports/bindings.ts";
import {
  findIdentifierWithAncestorKind,
  requireNode,
  type SemanticCodemodRoot,
} from "../_shared.ts";

export default function transform(root: SemanticCodemodRoot) {
  const usage = findIdentifierWithAncestorKind(root.root(), "Grid", "call_expression");
  const resolvedUsage = requireNode(usage, "Should find catch parameter usage");
  const shadow = findShadowingBinding(resolvedUsage);
  const resolvedShadow = requireNode(shadow, "Should treat catch parameter as shadowing");
  assert(resolvedShadow.text() === "Grid", "Catch parameter shadow should resolve to Grid");
  assert(
    !isRuntimeImportBinding(resolvedUsage),
    "Catch parameter should shadow the imported binding",
  );
  return null;
}

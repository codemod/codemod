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

  const resolvedUsage = requireNode(usage, "Should find post-catch usage");
  assert(
    findShadowingBinding(resolvedUsage) === null,
    "Catch parameter should not shadow outside the catch clause",
  );
  assert(
    isRuntimeImportBinding(resolvedUsage),
    "Post-catch usage should still resolve as the imported runtime binding",
  );
  return null;
}

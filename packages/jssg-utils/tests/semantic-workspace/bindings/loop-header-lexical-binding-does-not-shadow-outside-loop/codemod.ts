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

  const resolvedUsage = requireNode(usage, "Should find post-loop usage");
  assert(
    findShadowingBinding(resolvedUsage) === null,
    "Loop-header lexical binding should not shadow outside the loop scope",
  );
  assert(
    isRuntimeImportBinding(resolvedUsage),
    "Post-loop usage should still resolve as the imported runtime binding",
  );
  return null;
}

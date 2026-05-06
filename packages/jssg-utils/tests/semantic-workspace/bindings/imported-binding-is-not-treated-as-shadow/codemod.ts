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
      pattern: "makeStyles",
      inside: { kind: "call_expression" },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find imported helper usage");
  assert(
    findShadowingBinding(resolvedUsage) === null,
    "Import definitions should not be treated as shadowing bindings",
  );
  assert(
    isRuntimeImportBinding(resolvedUsage),
    "Imported helper usage should still resolve as a runtime import binding",
  );
  return null;
}

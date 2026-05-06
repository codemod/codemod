import { ok as assert } from "assert";
import { isRuntimeImportBinding } from "../../../../src/javascript/exports/bindings.ts";
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
    isRuntimeImportBinding(resolvedUsage),
    "Imported helper usage should still resolve as a runtime import binding",
  );
  return null;
}

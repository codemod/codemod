import { ok as assert } from "assert";
import { isRuntimeImportBinding } from "../../../../src/javascript/exports/bindings.ts";
import { requireNode, type SemanticCodemodRoot } from "../_shared.ts";

export default function transform(root: SemanticCodemodRoot) {
  const usage = root.root().find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: { kind: "object_assignment_pattern" },
    },
  });

  const resolvedUsage = requireNode(
    usage,
    "Should find imported runtime usage inside destructuring default initializer",
  );
  assert(
    isRuntimeImportBinding(resolvedUsage),
    "Destructuring default initializer usage should be treated as a runtime import binding",
  );
  return null;
}

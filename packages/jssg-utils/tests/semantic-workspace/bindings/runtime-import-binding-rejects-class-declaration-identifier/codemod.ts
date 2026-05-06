import { ok as assert } from "assert";
import { isRuntimeImportBinding } from "../../../../src/javascript/exports/bindings.ts";
import { requireNode, type SemanticCodemodRoot } from "../_shared.ts";

export default function transform(root: SemanticCodemodRoot) {
  const usage = root.root().find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: { kind: "class_declaration" },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find class declaration identifier");
  assert(
    !isRuntimeImportBinding(resolvedUsage),
    "Class declaration identifiers should not be treated as runtime usage",
  );
  return null;
}

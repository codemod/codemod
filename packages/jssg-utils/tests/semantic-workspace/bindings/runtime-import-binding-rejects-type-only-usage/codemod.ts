import { ok as assert } from "assert";
import { isRuntimeImportBinding } from "../../../../src/javascript/exports/bindings.ts";
import { requireNode, type SemanticCodemodRoot } from "../_shared.ts";

export default function transform(root: SemanticCodemodRoot) {
  const usage = root.root().find({
    rule: {
      kind: "identifier",
      pattern: "Grid",
      inside: { kind: "import_specifier" },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find type usage");
  assert(
    !isRuntimeImportBinding(resolvedUsage),
    "Type-only import usage should not be treated as runtime",
  );
  return null;
}

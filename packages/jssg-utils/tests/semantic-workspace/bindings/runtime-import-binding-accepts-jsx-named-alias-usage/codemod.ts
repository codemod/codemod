import { ok as assert } from "assert";
import { isRuntimeImportBinding } from "../../../../src/javascript/exports/bindings.ts";
import { requireNode, type SemanticCodemodRoot } from "../_shared.ts";

export default function transform(root: SemanticCodemodRoot) {
  const usage = root.root().find({
    rule: {
      kind: "identifier",
      pattern: "JoyGrid",
      inside: { kind: "jsx_self_closing_element" },
    },
  });

  const resolvedUsage = requireNode(usage, "Should find JSX JoyGrid usage");
  assert(
    isRuntimeImportBinding(resolvedUsage),
    "JSX tag identifier should resolve to a named aliased runtime import binding",
  );
  return null;
}

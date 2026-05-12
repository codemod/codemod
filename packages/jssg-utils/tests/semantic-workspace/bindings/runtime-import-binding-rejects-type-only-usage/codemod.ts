import { ok as assert } from "assert";
import { isRuntimeImportBinding } from "../../../../src/javascript/exports/bindings.ts";
import { requireNode, type SemanticCodemodRoot } from "../_shared.ts";

export default function transform(root: SemanticCodemodRoot) {
  const typeAlias = root.root().find({
    rule: {
      kind: "type_alias_declaration",
    },
  });
  const usage =
    typeAlias
      ?.children()
      .find((node) => String(node.kind()) === "type_identifier" && node.text() === "Grid") ?? null;

  const resolvedUsage = requireNode(usage, "Should find type usage");
  assert(
    !isRuntimeImportBinding(resolvedUsage),
    "Type-only import usage should not be treated as runtime",
  );
  return null;
}

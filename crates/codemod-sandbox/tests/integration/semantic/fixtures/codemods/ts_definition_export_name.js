// The definition lookup says how the declaration it lands on is exported,
// gives its name node, and goes from a use of an imported name to the
// declaration in one call.
import { symbols } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

function describe(def) {
  if (!def) return null;
  return {
    kind: def.kind,
    file: def.node.getRoot().filename().split("/").pop(),
    exportName: def.exportName,
    name: def.nameNode && def.nameNode.text(),
  };
}

export default function transform(root) {
  const { definition } = symbols("typescript");
  const def = (node) => definition.get(node)?.[0];
  const uses = root
    .root()
    .find({ rule: { kind: "array" } })
    .findAll({ rule: { kind: "identifier" } });
  const [modal, limit, formatValue] = uses;

  expectEqual(
    describe(def(modal)),
    { kind: "external", file: "modal.ts", exportName: "default", name: "Modal" },
    "export default function",
  );
  expectEqual(
    describe(def(limit)),
    { kind: "external", file: "consts.ts", exportName: "LIMIT", name: "LIMIT" },
    "export const with a type annotation, through export *",
  );
  expectEqual(
    describe(def(formatValue)),
    { kind: "external", file: "helpers.ts", exportName: "formatValue", name: "format" },
    "export { format as formatValue }",
  );

  // A local declaration that is exported has its export name too.
  const out = root.root().find({ rule: { pattern: "out" } });
  expectEqual(
    describe(def(out)),
    { kind: "local", file: "app.ts", exportName: "out", name: "out" },
    "local exported const",
  );
  return null;
}

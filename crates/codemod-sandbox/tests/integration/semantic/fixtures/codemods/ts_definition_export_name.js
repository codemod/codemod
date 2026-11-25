// definition() says how the declaration it lands on is exported, gives its
// name node, and with { follow: true } goes from a use of an imported name to
// the declaration in one call.
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
    file: def.root.filename().split("/").pop(),
    exportName: def.exportName,
    name: def.nameNode && def.nameNode.text(),
  };
}

export default function transform(root) {
  const uses = root
    .root()
    .find({ rule: { kind: "array" } })
    .findAll({ rule: { kind: "identifier" } });
  const [modal, limit, formatValue] = uses;

  // Without follow: the import binding in this file, not exported.
  expectEqual(
    describe(modal.definition()),
    { kind: "local", file: "app.ts", exportName: null, name: "Modal" },
    "Modal without follow",
  );

  expectEqual(
    describe(modal.definition({ follow: true })),
    { kind: "external", file: "modal.ts", exportName: "default", name: "Modal" },
    "export default function",
  );
  expectEqual(
    describe(limit.definition({ follow: true })),
    { kind: "external", file: "consts.ts", exportName: "LIMIT", name: "LIMIT" },
    "export const with a type annotation, through export *",
  );
  expectEqual(
    describe(formatValue.definition({ follow: true })),
    { kind: "external", file: "helpers.ts", exportName: "formatValue", name: "format" },
    "export { format as formatValue }",
  );

  // A local declaration that is exported has its export name too.
  const out = root.root().find({ rule: { pattern: "out" } });
  expectEqual(
    describe(out.definition()),
    { kind: "local", file: "app.ts", exportName: "out", name: "out" },
    "local exported const",
  );
  return null;
}

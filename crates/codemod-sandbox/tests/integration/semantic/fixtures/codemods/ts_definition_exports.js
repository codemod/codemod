// The definition lookup says how the declaration it lands on is exported,
// and gives its name node.
import { symbols } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const describe = (def) => ({
  kind: def.kind,
  file: def.node.getRoot().filename().split("/").pop(),
  exportName: def.exportName,
  name: def.nameNode && def.nameNode.text(),
});
// [Modal(), LIMIT, formatValue("a")]
const uses = (root) =>
  root
    .root()
    .find({ rule: { kind: "array" } })
    .findAll({ rule: { kind: "identifier" } });
const definition = (node) => describe(symbols("typescript").definition.get(node)[0]);

test("export default function has export name default", (root) => {
  equal(definition(uses(root)[0]), {
    kind: "external",
    file: "modal.ts",
    exportName: "default",
    name: "Modal",
  });
});

test("an annotated export const through export * keeps its name", (root) => {
  equal(definition(uses(root)[1]), {
    kind: "external",
    file: "consts.ts",
    exportName: "LIMIT",
    name: "LIMIT",
  });
});

test("export { format as formatValue } has the exported name and the declared name node", (root) => {
  equal(definition(uses(root)[2]), {
    kind: "external",
    file: "helpers.ts",
    exportName: "formatValue",
    name: "format",
  });
});

test("an exported local declaration has its export name too", (root) => {
  equal(definition(root.root().find({ rule: { pattern: "out" } })), {
    kind: "local",
    file: "app.ts",
    exportName: "out",
    name: "out",
  });
});

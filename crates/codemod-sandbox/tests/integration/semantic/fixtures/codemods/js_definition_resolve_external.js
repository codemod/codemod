// An import from an external package (not in the workspace) can't be
// followed, so definition.get(node) lands on the import binding in this file:
// from the imported name it is the import itself (kind "import"), and from a
// use of the name it is the same import specifier, as the local declaration
// of the name (kind "local").
import { symbols } from "codemod:ast-grep";

function describe(def) {
  if (!def) return null;
  return {
    kind: def.kind,
    file: def.node.getRoot().filename().split("/").pop(),
    node: def.node.kind(),
    text: def.node.text(),
    exportName: def.exportName,
    name: def.nameNode && def.nameNode.text(),
  };
}

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const { definition } = symbols("javascript");
  const [imported, use] = root.root().findAll({ rule: { pattern: "something" } });
  if (!imported || !use) {
    throw new Error("Expected to find the import of 'something' and its use");
  }

  const fromImport = definition.get(imported);
  expectEqual(fromImport && fromImport.length, 1, "edges from the imported name");
  expectEqual(
    describe(fromImport[0]),
    {
      kind: "import",
      file: "input.js",
      node: "import_specifier",
      text: "something",
      exportName: null,
      name: "something",
    },
    "definition of the imported name",
  );

  const fromUse = definition.get(use);
  expectEqual(fromUse && fromUse.length, 1, "edges from the use");
  expectEqual(
    describe(fromUse[0]),
    {
      kind: "local",
      file: "input.js",
      node: "import_specifier",
      text: "something",
      exportName: null,
      name: "something",
    },
    "definition of a use",
  );
  if (!fromUse[0].node.equals(fromImport[0].node)) {
    throw new Error("Expected the use and the import to land on the same import specifier");
  }

  return null;
}

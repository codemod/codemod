// The definition and references lookups within one file.
import { symbols } from "codemod:ast-grep";
import { equal, test, throws } from "./spec.js";
export { run as default } from "./spec.js";

const declaredName = (root, name) =>
  root
    .root()
    .find({ rule: { kind: "variable_declarator", has: { field: "name", regex: `^${name}$` } } })
    .field("name");

// A definition edge, with the file its node is in.
function describe(def) {
  return {
    kind: def.kind,
    file: def.node.getRoot().filename().split("/").pop(),
    node: def.node.kind(),
    text: def.node.text(),
    exportName: def.exportName,
    name: def.nameNode && def.nameNode.text(),
  };
}

test("references of a variable are its uses, not its declaration", (root) => {
  const refs = symbols("javascript").references.get(declaredName(root, "counter")) ?? [];
  equal(
    refs.map((e) => e.node.getRoot().filename().split("/").pop() + ":" + e.node.text()),
    ["input.js:counter", "input.js:counter", "input.js:counter"],
  );
});

test("definition of a use of a local variable lands on it, as local", (root) => {
  const use = root
    .root()
    .find({ rule: { pattern: "console.log(myVar)" } })
    .field("arguments")
    .child(1);
  const [def] = symbols("javascript").definition.get(use);
  equal([def.kind, def.node.equals(declaredName(root, "myVar"))], ["local", true]);
});

// An import from a package outside the workspace can't be followed, so the
// definition lands on the import specifier in this file.
test("definition of an external import's name is the import", (root) => {
  const [imported] = root.root().findAll({ rule: { pattern: "something" } });
  equal(symbols("javascript").definition.get(imported).map(describe), [
    {
      kind: "import",
      file: "input.js",
      node: "import_specifier",
      text: "something",
      exportName: null,
      name: "something",
    },
  ]);
});

test("definition of a use of an external import is the same specifier, as local", (root) => {
  const [imported, use] = root.root().findAll({ rule: { pattern: "something" } });
  const { definition } = symbols("javascript");
  const fromUse = definition.get(use);
  equal(
    fromUse.map((d) => d.kind),
    ["local"],
  );
  equal(fromUse[0].node.equals(definition.get(imported)[0].node), true, "same node");
});

test("module resolution needs workspace scope", async (root) => {
  const { resolveModule, specifierFor } = symbols("javascript");
  await throws(
    () => resolveModule(root, "./db.js"),
    "resolveModule(): resolveModule: module resolution needs workspace scope",
  );
  await throws(
    () => specifierFor(root, "./db.js"),
    "specifierFor(): specifierFor: module resolution needs workspace scope",
  );
});

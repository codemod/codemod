// The definition and references lookups across Python modules, in
// workspace scope, with the importing module processed first.
import { symbols } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const definitionOf = (root, pattern) => {
  const call = root.root().find({ rule: { pattern } });
  return symbols("python").definition.get(call.field("function"))[0];
};
const where = (node) => node.getRoot().filename().split("/").pop() + ":" + node.text();
// Each reference as file:text, sorted.
const referencesOf = (def) =>
  symbols("python")
    .references.get(def.nameNode ?? def.node)
    .map((e) => where(e.node))
    .sort();

test("definition of a call to an imported function lands on it in its module", (root) => {
  const def = definitionOf(root, "add(1, 2)");
  equal(where(def.nameNode ?? def.node), "utils.py:add");
});

// ty_ide counts the definition among the references. (Class references are
// covered in py_one_file.js: each Python references query costs a few
// hundred milliseconds.)
test("references of a function are its definition, import and calls in every module", (root) => {
  equal(referencesOf(definitionOf(root, "calculate(2, 3)")), [
    "main.py:calculate",
    "main.py:calculate",
    "main.py:calculate",
    "utils.py:calculate",
    "utils.py:calculate",
  ]);
});

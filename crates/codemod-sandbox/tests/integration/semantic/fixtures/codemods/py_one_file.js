// The references lookup within one Python file, in file scope. ty_ide
// counts the definition among the references. (Function references are
// covered across modules in py_cross_file.js: each Python references query
// costs a few hundred milliseconds.)
import { symbols } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const references = (root, pattern) =>
  (symbols("python").references.get(root.root().find({ rule: { pattern } })) ?? []).map((e) =>
    e.node.text(),
  );

test("references of an assignment are every assignment and read of the name", (root) => {
  equal(references(root, "counter = 0"), ["counter", "counter", "counter", "counter", "counter"]);
});

test("references of a class name are its definition, instantiations and isinstance", (root) => {
  equal(references(root, "Counter"), ["Counter", "Counter", "Counter", "Counter"]);
});

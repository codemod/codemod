// The references lookup only reaches modules that import the name from the
// module that declares it: really_imports.py imports my_var from app, so it
// is among app.py's references and not among models.py's.
import { symbols } from "codemod:ast-grep";
import { equal, onFile, test } from "./spec.js";
export { run as default } from "./spec.js";

// The files the references of the first my_var land in, sorted.
const filesReferring = (root) => {
  const myVar = root.root().find({ rule: { pattern: "my_var" } });
  const files = (symbols("python").references.get(myVar) ?? []).map((e) =>
    e.node.getRoot().filename().split("/").pop(),
  );
  return [...new Set(files)].sort();
};

onFile("app.py", () => {
  test("references reach a module importing the name under an alias", (root) => {
    equal(filesReferring(root), ["app.py", "really_imports.py"]);
  });
});

onFile("models.py", () => {
  test("references skip a module importing the same name from elsewhere", (root) => {
    equal(filesReferring(root), ["models.py"]);
  });
});

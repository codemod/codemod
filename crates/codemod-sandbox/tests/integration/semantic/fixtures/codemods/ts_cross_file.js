// The definition and references lookups across files, in workspace scope,
// with the importing file processed before the codemod runs.
import { symbols } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const where = (node) => node.getRoot().filename().split("/").pop() + ":" + node.text();
const addCall = (root) =>
  root
    .root()
    .find({ rule: { pattern: "add(1, 2)" } })
    .field("function");

test("definition of a call to an imported function lands on its export", (root) => {
  const [def] = symbols("typescript").definition.get(addCall(root));
  equal(
    [def.kind, where(def.node), def.node.parent().kind()],
    ["external", "utils.ts:add", "function_declaration"],
  );
});

test("references of an export are its import and its calls in other files", (root) => {
  const { definition, references } = symbols("typescript");
  const [def] = definition.get(addCall(root));
  equal(
    references.get(def.node).map((e) => where(e.node)),
    ["main.ts:add", "main.ts:add", "main.ts:add"],
  );
});

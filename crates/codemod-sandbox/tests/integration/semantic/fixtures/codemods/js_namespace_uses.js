// Uses through `import * as utils` count as references and as callers, or a
// safe-delete codemod deletes functions that are still used.
import { callGraph, symbols } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const name = (root, fn) =>
  root
    .root()
    .find({ rule: { kind: "function_declaration", has: { field: "name", regex: "^" + fn + "$" } } })
    .field("name");
const refs = (root, fn) =>
  (symbols("javascript").references.get(name(root, fn)) ?? []).map(
    (e) => e.node.getRoot().filename().split("/").pop() + ":" + e.node.text(),
  );
// Who makes a call: the calling function, or "<module>" for a call at a
// module's top level, where the edge's node is the call itself.
const callerName = (edge) =>
  edge.node.equals(edge.site) ? "<module>" : (edge.node.field("name")?.text() ?? "<anonymous>");
const callers = (root, fn) =>
  (callGraph("javascript").callers.get(name(root, fn)) ?? []).map(
    (e) => callerName(e) + ":" + e.via + " " + e.site.text(),
  );

test("a function stored in an object is a reference, not a call", (root) => {
  equal([refs(root, "oldRound"), callers(root, "oldRound")], [["app.js:oldRound"], []]);
});

test("a function handed to unknown code is a reference and an assumed callback", (root) => {
  equal(
    [refs(root, "registered"), callers(root, "registered")],
    [["app.js:registered"], ["<module>:callback hooks.register(utils.registered)"]],
  );
});

test("a computed namespace call is a reference and a method call", (root) => {
  equal(
    [refs(root, "called"), callers(root, "called")],
    [["app.js:called"], ['run:method utils?.["called"]()']],
  );
});

test("an unused export has neither references nor callers", (root) => {
  equal([refs(root, "unused"), callers(root, "unused")], [[], []]);
});

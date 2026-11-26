// Class inheritance in the callees lookup: methods and constructors up the
// extends chain, super(...) and super.m(...), across modules.
import { callGraph } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const callees = (node) =>
  (callGraph("javascript").callees.get(node) ?? []).map(
    (e) =>
      e.node.field("name").text() +
      "@" +
      e.node.getRoot().filename().split("/").pop() +
      ":" +
      e.via,
  );
const method = (root, name) =>
  root
    .root()
    .find({ rule: { kind: "method_definition", has: { field: "name", regex: "^" + name + "$" } } })
    .field("name");
const call = (root, pattern) => root.root().find({ rule: { pattern } });

test("super(...) calls the parent's constructor", (root) => {
  equal(callees(call(root, "super(1)")), ["constructor@base.js:constructor"]);
});

test("super.m() calls the parent's method, not the override", (root) => {
  equal(callees(call(root, "super.describe()")), ["describe@base.js:method"]);
});

test("this.m() calls an inherited method, or the class's own override", (root) => {
  equal(callees(method(root, "run")), ["greet@base.js:method", "describe@main.js:method"]);
});

// new Leaf() has no constructor of its own: Sub's runs; leaf.greet() is
// Base's; new Mixed() extends a call result, so it has no callee.
test("new and method calls resolve up the extends chain, not through a mixin", (root) => {
  equal(callees(root.root().find({ rule: { kind: "function_declaration" } })).sort(), [
    "constructor@main.js:constructor",
    "go@main.js:method",
    "greet@base.js:method",
  ]);
});

test("a class extending a call result does not know its parent's methods", (root) => {
  equal(callGraph("javascript").callees.has(method(root, "go")), false);
});

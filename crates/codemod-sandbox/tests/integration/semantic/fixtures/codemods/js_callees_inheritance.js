// Class inheritance in the callees lookup: methods and constructors up the
// extends chain, super(...) and super.m(...), across modules.
import { callGraph } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

function basename(path) {
  return path.split("/").pop();
}

export default function transform(root) {
  const { callees } = callGraph("javascript");
  const describe = (node) =>
    (callees.get(node) ?? []).map(
      (e) => e.node.field("name").text() + "@" + basename(e.node.getRoot().filename()) + ":" + e.via,
    );
  const method = (name) =>
    root
      .root()
      .find({
        rule: { kind: "method_definition", has: { field: "name", regex: "^" + name + "$" } },
      })
      .field("name");
  const call = (pattern) => root.root().find({ rule: { pattern } });

  // super(1) is Base's constructor.
  expectEqual(describe(call("super(1)")), ["constructor@base.js:constructor"], "super(1)");
  // super.describe() is Base's describe, not Sub's own.
  expectEqual(describe(call("super.describe()")), ["describe@base.js:method"], "super.describe()");
  // this.greet() is inherited; this.describe() is Sub's override.
  expectEqual(
    describe(method("run")),
    ["greet@base.js:method", "describe@main.js:method"],
    "callees of run",
  );
  // new Leaf() has no constructor of its own: Sub's runs. leaf.greet() is
  // Base's. A class extending a call result (mixin) does not know its
  // parent: new Mixed() has no callee.
  expectEqual(
    describe(root.root().find({ rule: { kind: "function_declaration" } })).sort(),
    ["constructor@main.js:constructor", "go@main.js:method", "greet@base.js:method"],
    "callees of main",
  );
  // So this.greet() inside it has no callee either.
  expectEqual(callees.has(method("go")), false, "callees of go");
  return null;
}

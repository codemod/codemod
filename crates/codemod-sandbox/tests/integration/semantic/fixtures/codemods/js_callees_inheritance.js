// Class inheritance: methods and constructors up the extends chain, super(...)
// and super.m(...), across modules.
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

function describe(entries) {
  return entries.map((c) =>
    c.resolved
      ? c.name + "@" + basename(c.root.filename()) + ":" + c.via
      : "u:" + c.name + ":" + c.via,
  );
}

export default function transform(root) {
  const method = (name) =>
    root
      .root()
      .find({
        rule: { kind: "method_definition", has: { field: "name", regex: "^" + name + "$" } },
      })
      .field("name");
  const call = (pattern) => root.root().find({ rule: { pattern } });

  // super(1) is Base's constructor.
  expectEqual(
    describe(call("super(1)").callees()),
    ["constructor@base.js:constructor"],
    "super(1)",
  );
  // super.describe() is Base's describe, not Sub's own.
  expectEqual(
    describe(call("super.describe()").callees()),
    ["describe@base.js:method"],
    "super.describe()",
  );
  // this.greet() is inherited; this.describe() is Sub's override.
  expectEqual(
    describe(method("run").callees()),
    ["greet@base.js:method", "describe@main.js:method"],
    "callees of run",
  );
  // new Leaf() has no constructor of its own: Sub's runs. leaf.greet() is
  // Base's. Can't tell: a class extending a call result (mixin) does not
  // know its parent, so this.greet() inside it stays unresolved.
  expectEqual(
    describe(
      root
        .root()
        .find({ rule: { kind: "function_declaration" } })
        .callees(),
    ).sort(),
    [
      "constructor@main.js:constructor",
      "go@main.js:method",
      "greet@base.js:method",
      "u:Mixed:constructor",
    ],
    "callees of main",
  );
  expectEqual(describe(method("go").callees()), ["u:this.greet:method"], "callees of go");
  return null;
}

// x.put() where x: Store, an interface: the call may reach every class
// member that implements it. Needs a TypeScript 7 checker (CODEMOD_TS_API).
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const call = root.root().find({ rule: { pattern: "store.put($$$)" } });
  const callees = call.callees();
  expectEqual(
    callees.map((c) => [c.node.text(), c.via, c.resolved]).sort(),
    [
      ["(key: string) => {}", "type", true],
      ["put(key: string) {}", "type", true],
    ],
    "callees of store.put()",
  );
  const store = call.find({ rule: { pattern: "store" } });
  expectEqual(store.typeInfo(), "Store", "type of store");

  // list.push(): unresolved (library code), with its declaration.
  const push = root.root().find({ rule: { pattern: "list.push($$$)" } });
  const [entry] = push.callees();
  expectEqual([entry.resolved, entry.via], [false, "method"], "list.push() stays unresolved");
  if (!entry.declaration) throw new Error("list.push(): no declaration");
  expectEqual(
    [entry.declaration.name, entry.declaration.root.filename().split("/").pop()],
    ["push", "lib.es5.d.ts"],
    "declaration of push",
  );
  if (!entry.declaration.node.text().startsWith("push(")) {
    throw new Error("unexpected declaration " + entry.declaration.node.text());
  }
  return null;
}

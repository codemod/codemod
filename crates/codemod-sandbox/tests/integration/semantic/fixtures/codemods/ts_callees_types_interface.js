// x.put() where x: Store, an interface: the callees lookup may reach every
// class member that implements it, and typeOf gives the interface. Needs a
// TypeScript 7 checker (CODEMOD_TS_API).
import { callGraph, types } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const { callees } = callGraph("typescript");
  const { typeOf } = types("typescript");
  const call = root.root().find({ rule: { pattern: "store.put($$$)" } });
  expectEqual(
    callees
      .get(call)
      .map((e) => [e.node.text(), e.via])
      .sort(),
    [
      ["(key: string) => {}", "type"],
      ["put(key: string) {}", "type"],
    ],
    "callees of store.put()",
  );
  const store = call.find({ rule: { pattern: "store" } });
  expectEqual(typeOf.get(store), "Store", "type of store");

  // list.push(): library code, no workspace function, so no edge.
  const push = root.root().find({ rule: { pattern: "list.push($$$)" } });
  expectEqual(callees.has(push), false, "list.push() has no callee");
  return null;
}

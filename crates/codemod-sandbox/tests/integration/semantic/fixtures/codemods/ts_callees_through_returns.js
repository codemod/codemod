// A call's result is what the called function returns: tuples
// (const [s, setS] = createSignal()), objects, f.bind(…).
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const callee = (pattern) =>
    root
      .root()
      .find({ rule: { pattern } })
      .callees()
      .map((c) => (c.resolved ? c.name + ":" + c.via : "u:" + c.name));

  // setSignal.bind(null, node) is setSignal; accessor() returns read.bind(…).
  expectEqual(callee("setCount($A)"), ["setSignal:direct"], "setCount");
  expectEqual(callee("count()"), ["read:direct"], "count");
  expectEqual(callee("store.set(2)"), ["set:method"], "store.set");
  expectEqual(callee("get()"), ["get:direct"], "get");
  // A getter's value is what it returns, and reading it is not handing the
  // getter function itself to log().
  expectEqual(callee("store.handler()"), ["onEvent:method"], "store.handler()");
  expectEqual(callee("log($A)"), ["u:log"], "log(store.label)");
  // Can't tell: an element pushed at run time, and a package outside the
  // workspace.
  expectEqual(callee("first()"), ["u:first"], "first");
  expectEqual(callee('getByRole("button")'), ["u:getByRole"], "getByRole");
  return null;
}

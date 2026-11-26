// The callees lookup follows a call's result to what the called function
// returns: tuples (const [s, setS] = createSignal()), objects, f.bind(…).
import { callGraph } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

// A function's name: its own, or the name it is bound to.
function nameOf(fn) {
  const own = fn.field("name");
  if (own) return own.text();
  const parent = fn.parent();
  const bound = parent && (parent.field("name") || parent.field("key"));
  return bound ? bound.text() : "<anonymous>";
}

export default function transform(root) {
  const { callees } = callGraph("typescript");
  const callee = (pattern) =>
    (callees.get(root.root().find({ rule: { pattern } })) ?? []).map((e) => nameOf(e.node) + ":" + e.via);

  // setSignal.bind(null, node) is setSignal; accessor() returns read.bind(…).
  expectEqual(callee("setCount($A)"), ["setSignal:direct"], "setCount");
  expectEqual(callee("count()"), ["read:direct"], "count");
  expectEqual(callee("store.set(2)"), ["set:method"], "store.set");
  expectEqual(callee("get()"), ["get:direct"], "get");
  // A getter's value is what it returns, and reading it is not handing the
  // getter function itself to log(): log(store.label) has no edge.
  expectEqual(callee("store.handler()"), ["onEvent:method"], "store.handler()");
  expectEqual(callee("log($A)"), [], "log(store.label)");
  // Can't tell: an element pushed at run time, and a package outside the
  // workspace. Neither has an edge.
  expectEqual(callee("first()"), [], "first");
  expectEqual(callee('getByRole("button")'), [], "getByRole");
  return null;
}

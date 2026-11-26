// The callees lookup follows a call's result to what the called function
// returns: tuples (const [s, setS] = createSignal()), objects, f.bind(…).
import { callGraph } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

// A function's name: its own, or the name it is bound to.
function nameOf(fn) {
  const own = fn.field("name");
  if (own) return own.text();
  const parent = fn.parent();
  const bound = parent && (parent.field("name") || parent.field("key"));
  return bound ? bound.text() : "<anonymous>";
}
const callee = (root, pattern) =>
  (callGraph("typescript").callees.get(root.root().find({ rule: { pattern } })) ?? []).map(
    (e) => nameOf(e.node) + ":" + e.via,
  );

test("a bound function returned in a tuple is the function it binds", (root) => {
  equal(callee(root, "setCount($A)"), ["setSignal:direct"]);
});

test("a function returning f.bind(…) returns f", (root) => {
  equal(callee(root, "count()"), ["read:direct"]);
});

test("a method of a returned object resolves", (root) => {
  equal(callee(root, "store.set(2)"), ["set:method"]);
  equal(callee(root, "get()"), ["get:direct"], "destructured");
});

test("a getter's value is what it returns, not the getter", (root) => {
  equal(callee(root, "store.handler()"), ["onEvent:method"]);
  equal(callee(root, "log($A)"), [], "log(store.label)");
});

test("an element pushed at run time has no callee", (root) => {
  equal(callee(root, "first()"), []);
});

test("a function from a package outside the workspace has no callee", (root) => {
  equal(callee(root, 'getByRole("button")'), []);
});

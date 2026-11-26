// With the TypeScript 7 checker: calls resolve through the receiver's type
// (a class, or an interface's implementations), and the typeOf lookup gives
// the checker's type of an expression. Needs CODEMOD_TS_API.
import { callGraph, crawl, types } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const name = (node) => node.field("name")?.text();
const register = (root) => root.root().find({ rule: { kind: "function_declaration" } });
const saveCall = (root) => register(root).find({ rule: { pattern: "repo.save($$$)" } });
const putCall = (root) => root.root().find({ rule: { pattern: "store.put($$$)" } });
// Hops back to the start along `from`.
function depthOf(reached, node) {
  let depth = 0;
  for (let edge = reached.get(node); edge; edge = reached.get(edge.from)) depth++;
  return depth;
}

test("a method call on a class-typed receiver resolves through the type", (root) => {
  equal(
    callGraph("typescript")
      .callees.get(saveCall(root))
      .map((e) => [name(e.node), e.node.getRoot().filename().split("/").pop(), e.via]),
    [["save", "repo.ts", "type"]],
  );
});

test("a crawl continues through a typed edge", async (root) => {
  const all = await crawl(callGraph("typescript").callees, register(root));
  equal(
    [...all]
      .filter(([node, edge]) => edge && name(node) === "audit")
      .map(([node, edge]) => [edge.via, depthOf(all, node)]),
    [["direct", 2]],
  );
});

test("callers of a method include calls typed to its class", (root) => {
  const { callees, callers } = callGraph("typescript");
  const save = callees.get(saveCall(root))[0].node;
  equal(
    callers.get(save).map((e) => [name(e.node), e.via]),
    [["register", "type"]],
  );
});

test("stopAt what typed edges reached leaves the rest out of the walk", async (root) => {
  const { callees } = callGraph("typescript");
  const all = await crawl(callees, register(root));
  const typed = [...all].filter(([, edge]) => edge?.via === "type").map(([node]) => node);
  const untyped = await crawl(callees, register(root), { stopAt: typed });
  equal(
    [...untyped.keys()].filter((node) => name(node) === "audit"),
    [],
  );
});

test("a method call on an interface-typed receiver may reach every implementation", (root) => {
  equal(
    callGraph("typescript")
      .callees.get(putCall(root))
      .map((e) => [e.node.text(), e.via])
      .sort(),
    [
      ["(key: string) => {}", "type"],
      ["put(key: string) {}", "type"],
    ],
  );
});

test("a library method has no callee, even with types", (root) => {
  equal(
    callGraph("typescript").callees.has(root.root().find({ rule: { pattern: "list.push($$$)" } })),
    false,
  );
});

test("typeOf gives the checker's type of an expression", (root) => {
  const { typeOf } = types("typescript");
  const call = saveCall(root);
  equal(
    [
      typeOf.get(call.find({ rule: { pattern: "repo" } })),
      typeOf.get(call),
      typeOf.get(call.find({ rule: { kind: "object" } })),
      typeOf.get(putCall(root).find({ rule: { pattern: "store" } })),
    ],
    ["UserRepo", "void", "{ id: string; }", "Store"],
  );
});

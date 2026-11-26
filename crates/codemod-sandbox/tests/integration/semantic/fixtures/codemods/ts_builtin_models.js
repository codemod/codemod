// Models of built-in methods: Promise then/catch, Array callbacks and
// dynamic import(). Fixture: the debarrel review's dynamic-import probe
// (consumer.ts) plus models.ts.
import { callGraph, crawl, dataflow } from "codemod:ast-grep";
import { equal, excludes, includes, onFile, test } from "./spec.js";
export { run as default } from "./spec.js";

const describe = (edges) =>
  edges.map((s) => s.kind + ":" + s.node.text() + (s.boundary ? "!" : "")).sort();
const find = (root, pattern) => {
  const node = root.root().find({ rule: { pattern } });
  if (!node) throw new Error(pattern + " not found");
  return node;
};
const argOf = (root, pattern) =>
  find(root, pattern)
    .field("arguments")
    .children()
    .filter((n) => n.isNamed())[0];
const sourcesOf = async (node, options) =>
  [...(await crawl(dataflow("typescript").sources, node, options)).values()].filter((e) => e);

onFile("src/consumer.ts", () => {
  test("a dynamic import's module flows into then's parameter and its members", async (root) => {
    const reached = await crawl(dataflow("typescript").uses, find(root, 'import("./lib")'));
    const forward = describe([...reached.values()].filter((edge) => edge));
    includes(forward, "parameter:m");
    includes(forward, "member:m.b");
  });
});

onFile("src/models.ts", () => {
  test("filter keeps elements: its callback's parameter comes from the array", async (root) => {
    includes(describe(await sourcesOf(argOf(root, "audit($A)"))), "parameter:orders!");
  });

  test("then's parameter is what the promise resolves to", async (root) => {
    includes(describe(await sourcesOf(argOf(root, "send($A)"))), "call:fetchRate()!");
  });

  test("reduce's accumulator comes from its seed and each element", async (root) => {
    const sent = describe(await sourcesOf(argOf(root, "send($A)")));
    includes(sent, "literal:0");
    includes(sent, "member:order.amount");
  });

  // A second crawl stops at every call the first one reached, and at every
  // parameter a model takes from a call (rate, from fetchRate().then(...)).
  test("stopAt calls and modeled parameters ends the trail at the parameter", async (root) => {
    const { sources } = dataflow("typescript");
    const fromSent = await crawl(sources, argOf(root, "send($A)"));
    const stops = [];
    for (const [node, edge] of fromSent) {
      if (edge?.kind !== "call") continue;
      stops.push(node);
      if (fromSent.get(edge.from)?.kind === "parameter") stops.push(edge.from);
    }
    const shallow = describe(await sourcesOf(argOf(root, "send($A)"), { stopAt: stops }));
    includes(shallow, "parameter:rate!");
    excludes(shallow, "call:fetchRate()!");
    excludes(shallow, "literal:0");
  });

  test("a rejection reason stays a boundary", async (root) => {
    equal(describe(await sourcesOf(argOf(root, "report($A)"))), ["parameter:err!"]);
  });

  test("a named callback's parameter stays a boundary", async (root) => {
    equal(describe(await sourcesOf(argOf(root, "record($A)"))), ["parameter:rate!"]);
  });

  test("calls through a dynamic import's namespace resolve", (root) => {
    for (const call of ["c()", "mod.c()"]) {
      equal(
        (callGraph("typescript").callees.get(find(root, call)) ?? []).map(
          (e) => e.node.field("name").text() + "@" + e.node.getRoot().filename().split("/").pop(),
        ),
        ["c@c.ts"],
        call,
      );
    }
  });
});

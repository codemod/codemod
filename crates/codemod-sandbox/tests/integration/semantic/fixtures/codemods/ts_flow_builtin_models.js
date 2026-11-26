// Builtin method models: Promise then/catch and Array callbacks. Fixture:
// the debarrel review's dynamic-import probe (consumer.ts) plus models.ts.
// Backward flow is a crawl of the sources lookup, forward flow a crawl of
// the uses lookup; calls resolve in the callees lookup.
import { callGraph, crawl, dataflow } from "codemod:ast-grep";

function describe(steps) {
  return steps.map((s) => s.kind + ":" + s.node.text() + (s.boundary ? "!" : "")).sort();
}

function expectHas(list, entry, label) {
  if (!list.includes(entry)) {
    throw new Error(label + ": expected " + entry + " in " + JSON.stringify(list));
  }
}

function expectLacks(list, entry, label) {
  if (list.includes(entry)) {
    throw new Error(label + ": did not expect " + entry + " in " + JSON.stringify(list));
  }
}

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

export default async function transform(root) {
  const file = basename(root.filename());
  const find = (pattern) => {
    const node = root.root().find({ rule: { pattern } });
    if (!node) throw new Error(pattern + " not found");
    return node;
  };
  const argOf = (pattern) =>
    find(pattern)
      .field("arguments")
      .children()
      .filter((n) => n.isNamed())[0];

  const { sources, uses } = dataflow("typescript");
  // Every edge a crawl of the sources lookup reached (the start maps to null).
  const sourcesOf = async (node, options) =>
    [...(await crawl(sources, node, options)).values()].filter((e) => e);

  if (file === "consumer.ts") {
    // import("./lib").then((m) => m.b + a): the module reaches m, then m.b.
    const reached = await crawl(uses, find('import("./lib")'));
    const forward = describe([...reached.values()].filter((edge) => edge));
    expectHas(forward, "parameter:m", "import() flows to");
    expectHas(forward, "member:m.b", "import() flows to");
    return null;
  }
  if (file !== "models.ts") throw new Error("unexpected target " + file);

  // filter keeps elements: o is an element of orders.
  expectHas(describe(await sourcesOf(argOf("audit($A)"))), "parameter:orders!", "audit(o.id)");

  // rate is what fetchRate() resolves to; sum is reduce's accumulator,
  // seeded with 0 and fed by each order.amount.
  const fromSent = await crawl(sources, argOf("send($A)"));
  const sent = describe([...fromSent.values()].filter((e) => e));
  expectHas(sent, "call:fetchRate()!", "send(sum * rate)");
  expectHas(sent, "literal:0", "send(sum * rate)");
  expectHas(sent, "member:order.amount", "send(sum * rate)");

  // Not looking through calls, models included: a second crawl stops at
  // every call the first one reached, and at every parameter whose value a
  // model takes from a call (rate, from fetchRate().then(...)).
  const stops = [];
  for (const [node, edge] of fromSent) {
    if (edge?.kind !== "call") continue;
    stops.push(node);
    if (fromSent.get(edge.from)?.kind === "parameter") stops.push(edge.from);
  }
  const shallow = describe(await sourcesOf(argOf("send($A)"), { stopAt: stops }));
  expectHas(shallow, "parameter:rate!", "shallow send");
  expectLacks(shallow, "call:fetchRate()!", "shallow send");
  expectLacks(shallow, "literal:0", "shallow send");

  // Can't tell: a rejection reason, and a named callback (its parameter
  // belongs to all its callers) stay boundaries.
  expectEqual(describe(await sourcesOf(argOf("report($A)"))), ["parameter:err!"], "report(err)");
  expectEqual(describe(await sourcesOf(argOf("record($A)"))), ["parameter:rate!"], "record(rate)");

  // Calls through a dynamic import's namespace resolve (an unresolved call
  // would have no edge).
  const { callees } = callGraph("typescript");
  for (const call of ["c()", "mod.c()"]) {
    expectEqual(
      (callees.get(find(call)) ?? []).map(
        (e) => e.node.field("name").text() + "@" + basename(e.node.getRoot().filename()),
      ),
      ["c@c.ts"],
      call,
    );
  }
  return null;
}

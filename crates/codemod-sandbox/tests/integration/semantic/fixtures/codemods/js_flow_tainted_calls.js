// "Every call that receives a value derived from req.body": crawl the uses
// lookup from req.body, and collect the argument edges leaving the nodes it
// walked through.
import { crawl, dataflow } from "codemod:ast-grep";

const { uses } = dataflow("javascript");

async function callsReceiving(source, { throughCalls = true } = {}) {
  let reached = await crawl(uses, source);
  let stops = [];
  if (!throughCalls) {
    // Stop at each call the value is passed to: what the call returns is
    // not followed.
    stops = [...reached.values()].filter((e) => e?.via === "argument").map((e) => e.node);
    reached = await crawl(uses, source, { stopAt: stops });
  }
  const calls = new Set();
  for (const node of reached.keys()) {
    if (stops.some((stop) => stop.equals(node))) continue;
    for (const edge of uses.get(node) ?? []) {
      if (edge.via === "argument") calls.add(edge.node.text() + "#" + edge.index);
    }
  }
  return [...calls].sort();
}

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default async function transform(root) {
  const body = root.root().find({ rule: { pattern: "req.body" } });
  if (!body) throw new Error("req.body not found");

  // Calls whose result depends on a tainted argument pass the taint on:
  // `user` comes from createUser(label, email), so log(user) and
  // res.json(user) receive it too. query(...) only sees req.params and req.query.
  expectEqual(
    await callsReceiving(body),
    ["createUser(label, email)#0", "createUser(label, email)#1", "log(user)#0", "res.json(user)#0"],
    "derived from req.body",
  );

  // Only the calls that receive the value directly: the crawl stops at them.
  expectEqual(
    await callsReceiving(body, { throughCalls: false }),
    ["createUser(label, email)#0", "createUser(label, email)#1"],
    "directly from req.body",
  );

  return null;
}

// "Where does this argument come from?" A crawl of the sources lookup from
// the argument: every node the value came from, nearest first, each with the
// edge that reached it.
import { crawl, dataflow } from "codemod:ast-grep";

function describe(reached) {
  return [...reached.values()]
    .filter((e) => e)
    .map((e) => e.kind + ":" + e.node.text() + (e.boundary ? "!" : ""))
    .sort();
}

function expectHas(list, entry, label) {
  if (!list.includes(entry)) {
    throw new Error(label + ": expected " + entry + " in " + JSON.stringify(list));
  }
}

export default async function transform(root) {
  const { sources } = dataflow("javascript");
  const call = root.root().find({ rule: { pattern: "query($$$ARGS)" } });
  if (!call) throw new Error("query(...) call not found");
  const args = call
    .field("arguments")
    .children()
    .filter((n) => n.isNamed());

  // Second argument: `limit`, assigned twice.
  const fromLimit = await crawl(sources, args[1]);
  const limit = describe(fromLimit);
  expectHas(limit, "variable:limit", "limit");
  expectHas(limit, "literal:10", "limit");
  expectHas(limit, "call:Number(req.query.limit)!", "limit");
  expectHas(limit, "member:req.query.limit", "limit");
  expectHas(limit, "parameter:req!", "limit");

  // Nodes are reached nearest first, each with the edge that reached it; the
  // nearest one was reached from the start.
  const first = [...fromLimit.values()].find((e) => e);
  if (first.kind !== "variable" || first.via !== "read" || !first.from.equals(args[1])) {
    throw new Error(
      "expected the nearest node to be the limit binding, got " + first.kind + " via " + first.via,
    );
  }
  const param = [...fromLimit.values()].find((e) => e?.kind === "parameter");
  if (param.index !== 0 || param.name !== "req") {
    throw new Error("expected parameter req at index 0, got " + param.name + " at " + param.index);
  }

  // Not looking through calls: stop at every call the first crawl reached,
  // and Number(...) is where the trail ends.
  const calls = [...fromLimit.values()].filter((e) => e?.kind === "call").map((e) => e.node);
  const shallow = describe(await crawl(sources, args[1], { stopAt: calls }));
  expectHas(shallow, "call:Number(req.query.limit)!", "shallow limit");
  if (shallow.includes("parameter:req!")) {
    throw new Error("stopping at calls should stop at Number(...): " + JSON.stringify(shallow));
  }

  // First argument: a string built from `id`, which is req.params.id.
  const sql = describe(await crawl(sources, args[0]));
  expectHas(sql, "member:req.params.id", "sql");
  expectHas(sql, "parameter:req!", "sql");
  if (sql.some((s) => s.includes("req.body"))) {
    throw new Error("the sql argument does not depend on req.body: " + JSON.stringify(sql));
  }

  return null;
}

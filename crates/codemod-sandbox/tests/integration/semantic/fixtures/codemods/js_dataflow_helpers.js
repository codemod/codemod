// The helpers on top of the lookups: node identity across answers, what a
// crawl of the sources lookup depends on, what a crawl of the uses lookup
// reaches, the path back through edge.from, and edge.from on a crawl of the
// callers lookup.
import { callGraph, crawl, dataflow } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default async function transform(root) {
  const { sources, uses } = dataflow("javascript");
  const { callees, callers } = callGraph("javascript");
  const call = root.root().find({ rule: { pattern: "query($$$ARGS)" } });
  const [sql, limit] = call
    .field("arguments")
    .children()
    .filter((n) => n.isNamed());

  // Identity: the same node from two answers.
  const fromLimit = await crawl(sources, limit);
  const viaFlow = [...fromLimit.values()].find((e) => e?.kind === "variable").node;
  const declared = root
    .root()
    .find({ rule: { pattern: "let limit = 10" } })
    .find({ rule: { kind: "identifier" } });
  expectEqual(
    [viaFlow === declared, viaFlow.equals(declared), viaFlow.key() === declared.key()],
    [false, true, true],
    "identity",
  );
  expectEqual(viaFlow.equals(limit), false, "a different node");
  expectEqual(viaFlow.equals("limit"), false, "not a node");

  // Back from a key; containment across answers.
  const json = root.root().find({ rule: { pattern: "res.json(user)" } });
  const statement = json.parent();
  expectEqual(root.nodeByKey(json.key()).equals(json), true, "nodeByKey: a call");
  expectEqual(
    root.nodeByKey(statement.key()).kind(),
    "expression_statement",
    "nodeByKey: same start, other kind",
  );
  expectEqual(
    root.nodeByKey(json.key().replace("server.js", "other.js")),
    null,
    "nodeByKey: another file",
  );
  try {
    root.nodeByKey("not a key");
    throw new Error("expected a TypeError");
  } catch (error) {
    if (!String(error.message).includes("is not a node.key()")) throw error;
  }
  const handler = root.root().find({ rule: { kind: "function_declaration" } });
  expectEqual(
    [
      handler.contains(call),
      call.contains(handler),
      handler.contains(viaFlow),
      json.contains(json),
    ],
    [true, false, true, true],
    "contains",
  );

  // What a value depends on: the nodes and edges a crawl of the sources
  // lookup reaches.
  const dependsOn = async (node, test) =>
    [...(await crawl(sources, node))].some(([reached, edge]) => edge && test(reached, edge));
  expectEqual(
    await dependsOn(limit, (_, e) => e.kind === "parameter" && e.name === "req"),
    true,
    "limit depends on req",
  );
  expectEqual(await dependsOn(limit, (n) => n.equals(declared)), true, "limit depends on its variable");
  expectEqual(await dependsOn(limit, (_, e) => e.name === "res"), false, "not on res");
  expectEqual(
    await dependsOn(sql, (_, e) => e.kind === "call"),
    false,
    "no call in sql text",
  );
  // Where a value goes: the edges a crawl of the uses lookup reaches.
  const body = root.root().find({ rule: { pattern: "req.body" } });
  const reaches = async (node, test) =>
    [...(await crawl(uses, node)).values()].some((edge) => edge && test(edge));
  expectEqual(
    await reaches(body, (e) => e.via === "argument" && e.name === "createUser"),
    true,
    "body reaches createUser",
  );
  expectEqual(
    await reaches(body, (e) => e.via === "argument" && e.name === "query"),
    false,
    "not query",
  );
  // A crawl starts at a node (or its key), not at a name.
  try {
    await crawl(sources, "req");
    throw new Error("expected a TypeError");
  } catch (error) {
    if (!String(error.message).includes('"req" is not a node or a node.key()')) throw error;
  }

  // edge.from: the path back to the start. Each edge's from is the node the
  // next edge on the way back reached, and the last one's is the start,
  // which maps to null.
  const req = [...fromLimit.values()].find((e) => e?.kind === "parameter");
  const path = [];
  for (let e = req; e; e = fromLimit.get(e.from)) path.push(e);
  expectEqual(
    path.slice(0, -1).every((e, i) => e.from.equals(path[i + 1].node)),
    true,
    "each from is the next edge's node",
  );
  expectEqual(path[path.length - 1].from.equals(limit), true, "path ends at the start");
  expectEqual(fromLimit.get(limit), null, "the start has no edge");

  // edge.from on a crawl of the callers lookup: which function each caller
  // was reached from. connect is reached from query's call by crawling the
  // callees lookup.
  const fnName = (node) => node.field("name")?.text() ?? node.parent().field("name").text();
  const reachedFromCall = await crawl(callees, call);
  const connect = [...reachedFromCall.keys()].find(
    (node) => reachedFromCall.get(node) && fnName(node) === "connect",
  );
  if (!connect) throw new Error("connect not found in callees");
  {
    const reached = await crawl(callers, connect);
    const pairs = [...reached]
      .filter(([, edge]) => edge)
      .map(([node, edge]) => fnName(node) + "->" + fnName(edge.from))
      .sort();
    expectEqual(
      pairs.includes("query->connect") && pairs.includes("handler->query"),
      true,
      "callee names: " + pairs,
    );
  }
  return null;
}

// The helpers on top of the queries: node identity across answers,
// dependsOn / reaches, step.from, and callee on caller entries.
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const call = root.root().find({ rule: { pattern: "query($$$ARGS)" } });
  const [sql, limit] = call
    .field("arguments")
    .children()
    .filter((n) => n.isNamed());

  // Identity: the same node from two answers.
  const viaFlow = limit.flowsFrom().find((s) => s.kind === "variable").node;
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

  // Booleans.
  expectEqual(
    limit.dependsOn((s) => s.kind === "parameter" && s.name === "req"),
    true,
    "limit depends on req",
  );
  expectEqual(limit.dependsOn(declared), true, "limit depends on its variable");
  expectEqual(
    limit.dependsOn((s) => s.name === "res"),
    false,
    "not on res",
  );
  expectEqual(
    sql.dependsOn((s) => s.kind === "call", { throughCalls: false }),
    false,
    "no call in sql text",
  );
  const body = root.root().find({ rule: { pattern: "req.body" } });
  expectEqual(
    body.reaches((s) => s.via === "argument" && s.name === "createUser"),
    true,
    "body reaches createUser",
  );
  expectEqual(
    body.reaches((s) => s.via === "argument" && s.name === "query"),
    false,
    "not query",
  );
  try {
    limit.dependsOn("req");
    throw new Error("expected a TypeError");
  } catch (error) {
    if (!String(error.message).includes("must be an SgNode or a function")) throw error;
  }

  // step.from: the path back to the queried node.
  const req = limit.flowsFrom().find((s) => s.kind === "parameter");
  const path = [];
  for (let s = req; s; s = s.from) path.push(s.kind + ":" + s.depth);
  expectEqual(path[path.length - 1].endsWith(":1"), true, "path ends at depth 1");
  expectEqual(path.length, req.depth, "one step per depth");
  expectEqual(limit.flowsFrom()[0].from, null, "depth 1 has no from");

  // callee on caller entries: which function each call reaches.
  const save = call.callees({ transitive: true }).find((c) => c.resolved && c.name === "connect");
  if (!save) throw new Error("connect not found in callees");
  {
    const callers = save.node.callers({ transitive: true });
    const pairs = callers.map((c) => c.name + "->" + (c.callee && c.callee.name)).sort();
    expectEqual(
      pairs.includes("query->connect") && pairs.includes("handler->query"),
      true,
      "callee names: " + pairs,
    );
  }
  return null;
}

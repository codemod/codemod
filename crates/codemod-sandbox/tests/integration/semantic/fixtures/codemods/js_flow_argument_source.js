// "Where does this argument come from?"
function describe(steps) {
  return steps.map((s) => s.kind + ":" + s.node.text() + (s.boundary ? "!" : "")).sort();
}

function expectHas(list, entry, label) {
  if (!list.includes(entry)) {
    throw new Error(label + ": expected " + entry + " in " + JSON.stringify(list));
  }
}

export default function transform(root) {
  const call = root.root().find({ rule: { pattern: "query($$$ARGS)" } });
  if (!call) throw new Error("query(...) call not found");
  const args = call
    .field("arguments")
    .children()
    .filter((n) => n.isNamed());

  // Second argument: `limit`, assigned twice.
  const limit = describe(args[1].flowsFrom());
  expectHas(limit, "variable:limit", "limit");
  expectHas(limit, "literal:10", "limit");
  expectHas(limit, "call:Number(req.query.limit)!", "limit");
  expectHas(limit, "member:req.query.limit", "limit");
  expectHas(limit, "parameter:req!", "limit");

  // Steps are ordered nearest first and carry the edge that reached them.
  const first = args[1].flowsFrom()[0];
  if (first.kind !== "variable" || first.via !== "read" || first.depth !== 1) {
    throw new Error(
      "expected the nearest step to be the limit binding, got " + first.kind + " via " + first.via,
    );
  }
  const param = args[1].flowsFrom().find((s) => s.kind === "parameter");
  if (param.index !== 0 || param.name !== "req") {
    throw new Error("expected parameter req at index 0, got " + param.name + " at " + param.index);
  }

  // Without looking through calls, Number(...) is where the trail ends.
  const shallow = describe(args[1].flowsFrom({ throughCalls: false }));
  expectHas(shallow, "call:Number(req.query.limit)!", "shallow limit");
  if (shallow.includes("parameter:req!")) {
    throw new Error("throughCalls: false should stop at the call: " + JSON.stringify(shallow));
  }

  // First argument: a string built from `id`, which is req.params.id.
  const sql = describe(args[0].flowsFrom());
  expectHas(sql, "member:req.params.id", "sql");
  expectHas(sql, "parameter:req!", "sql");
  if (sql.some((s) => s.includes("req.body"))) {
    throw new Error("the sql argument does not depend on req.body: " + JSON.stringify(sql));
  }

  return null;
}

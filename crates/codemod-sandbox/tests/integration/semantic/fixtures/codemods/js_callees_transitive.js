// "Functions transitively called from handler", across modules.
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

export default function transform(root) {
  const handler = root.root().find({ rule: { kind: "function_declaration" } });
  if (!handler) throw new Error("handler not found");

  const direct = handler.callees();
  expectEqual(
    direct
      .filter((c) => c.resolved)
      .map((c) => c.name + "@" + basename(c.root.filename()))
      .sort(),
    ["createUser@users.js", "query@db.js"],
    "direct callees",
  );
  // Calls into code outside the workspace are reported, not dropped.
  expectEqual(
    direct
      .filter((c) => !c.resolved)
      .map((c) => c.name)
      .sort(),
    ["Number", "log", "res.json"],
    "unresolved direct calls",
  );

  const all = handler.callees({ transitive: true });
  expectEqual(
    all
      .filter((c) => c.resolved)
      .map((c) => c.name + "@" + basename(c.root.filename()) + ":" + c.via + ":" + c.depth)
      .sort(),
    [
      "connect@db.js:direct:2",
      "createUser@users.js:direct:1",
      "normalize@users.js:callback:2",
      "query@db.js:direct:1",
      "save@db.js:method:2",
      "validate@validate.js:direct:2",
    ],
    "transitive callees",
  );
  // Nothing was cut off, and every file the walk needed parsed.
  expectEqual([all.truncated, all.unparsed], [false, []], "transitive callees complete");

  // createUser is imported through the services/index.js barrel; the entry
  // points at the arrow function in users.js, and at the call that reaches it.
  const createUser = all.find((c) => c.name === "createUser");
  if (createUser.node.kind() !== "arrow_function") {
    throw new Error("expected the arrow function, got " + createUser.node.kind());
  }
  if (createUser.callSite.node.text() !== "createUser(label, email)") {
    throw new Error("unexpected call site " + createUser.callSite.node.text());
  }

  // The same graph, walked the other way from db.js's connect().
  const save = all.find((c) => c.name === "save");
  const connectCall = save.node.find({ rule: { pattern: "connect()" } });
  const callers = connectCall.callers({ transitive: true });
  expectEqual(
    callers.map((c) => c.name + ":" + c.depth).sort(),
    ["createUser:2", "handler:3", "handler:2", "query:1", "save:1"].sort(),
    "transitive callers of connect",
  );
  expectEqual([callers.truncated, callers.unparsed], [false, []], "callers complete");

  return null;
}

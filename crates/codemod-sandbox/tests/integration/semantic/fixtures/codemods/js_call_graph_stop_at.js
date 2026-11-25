// callees({ transitive, stopAt }): report edges of a kind, do not walk past them.
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const handler = root.root().find({ rule: { kind: "function_declaration" } });
  const resolved = (entries) =>
    entries
      .filter((c) => c.resolved)
      .map((c) => c.name + ":" + c.via + ":" + c.depth)
      .sort();

  // Callbacks are reported but not entered: normalize is listed, and the
  // rest of the walk is unchanged.
  const noCallbacks = resolved(handler.callees({ transitive: true, stopAt: ["callback"] }));
  expectEqual(
    noCallbacks,
    [
      "connect:direct:2",
      "createUser:direct:1",
      "normalize:callback:2",
      "query:direct:1",
      "save:method:2",
      "validate:direct:2",
    ],
    "stopAt callback",
  );

  // Method calls are reported but not entered: db.save is listed, connect
  // (called from save) is not reached through it.
  const noMethods = resolved(handler.callees({ transitive: true, stopAt: ["method"] }));
  if (!noMethods.includes("save:method:2")) throw new Error("save should be listed");
  if (noMethods.some((e) => e.startsWith("connect:") && e.endsWith(":3"))) {
    // connect is still reached from query (direct) at depth 2.
    throw new Error("connect should not be reached through save: " + JSON.stringify(noMethods));
  }
  return null;
}

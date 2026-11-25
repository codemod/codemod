// An overloaded function resolves to its implementation, not to the first
// signature. Found by comparing with Jelly on solid-router (createAsync).
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const main = root.root().find({ rule: { kind: "function_declaration" } });
  const all = main.callees({ transitive: true });
  expectEqual(
    all.map((c) =>
      c.resolved ? c.name + ":" + c.node.text().includes("return value") : "u:" + c.name,
    ),
    // Can't tell: a `declare function` has no body in the workspace.
    ["pick:true", "useAmbient:false", "u:ambient"],
    "callees of main",
  );
  return null;
}

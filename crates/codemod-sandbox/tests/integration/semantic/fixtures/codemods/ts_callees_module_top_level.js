// "Does evaluating this module run code?" The program node's callees are the
// calls made at module top level. Fixture: debarrel gap D (side-effect barrel).
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

function describe(entries) {
  return entries.map((c) => (c.resolved ? "R" : "u") + ":" + c.via + ":" + c.name + ":" + c.depth);
}

export default function transform(root) {
  const file = root.filename().split("/").pop();
  const program = root.root();

  if (file === "register.ts") {
    // registry.set(...) runs on import; registry is a Map, so the call is
    // reported unresolved rather than dropped.
    expectEqual(describe(program.callees()), ["u:method:registry.set:1"], "register.ts");
  } else if (file === "App.ts") {
    expectEqual(describe(program.callees()), ["R:direct:Icon:1"], "App.ts");
    expectEqual(
      describe(program.callees({ transitive: true })),
      ["R:direct:Icon:1", "u:method:registry.get:2"],
      "App.ts transitive",
    );
  } else if (file === "Icon.ts") {
    // A module that only declares a function runs nothing on import. It used
    // to answer with the function's callees.
    expectEqual(describe(program.callees()), [], "Icon.ts");
    const icon = program.find({ rule: { kind: "function_declaration" } });
    expectEqual(describe(icon.callees()), ["u:method:registry.get:1"], "Icon()");
  } else {
    throw new Error("unexpected target " + file);
  }
  return null;
}

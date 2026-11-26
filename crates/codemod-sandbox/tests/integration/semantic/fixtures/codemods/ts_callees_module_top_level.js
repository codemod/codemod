// "Does evaluating this module call a workspace function?" The callees
// lookup on the program node answers with the calls made at module top
// level. Fixture: debarrel gap D (side-effect barrel).
import { callGraph, crawl } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

// Hops back to a start node along `from`.
function depthOf(reached, node) {
  let depth = 0;
  for (let edge = reached.get(node); edge; edge = reached.get(edge.from)) depth++;
  return depth;
}

export default async function transform(root) {
  const { callees } = callGraph("typescript");
  const describe = (node) =>
    (callees.get(node) ?? []).map((e) => e.via + ":" + e.node.field("name").text());
  const file = root.filename().split("/").pop();
  const program = root.root();

  if (file === "register.ts") {
    // registry.set(...) runs on import, but registry is a Map: the call
    // resolves to no workspace function, so there is no edge.
    expectEqual(describe(program), [], "register.ts");
  } else if (file === "App.ts") {
    expectEqual(describe(program), ["direct:Icon"], "App.ts");
    // Icon calls registry.get(), a Map method: the crawl stops at Icon.
    const reached = await crawl(callees, program);
    expectEqual(
      [...reached]
        .filter(([, edge]) => edge)
        .map(([node, edge]) => edge.via + ":" + node.field("name").text() + ":" + depthOf(reached, node)),
      ["direct:Icon:1"],
      "App.ts crawled",
    );
  } else if (file === "Icon.ts") {
    // A module that only declares a function runs nothing on import. It used
    // to answer with the function's callees.
    expectEqual(describe(program), [], "Icon.ts");
    const icon = program.find({ rule: { kind: "function_declaration" } });
    expectEqual(describe(icon), [], "Icon(): registry.get() has no edge");
  } else {
    throw new Error("unexpected target " + file);
  }
  return null;
}

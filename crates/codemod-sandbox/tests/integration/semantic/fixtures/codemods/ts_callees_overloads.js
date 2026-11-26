// An overloaded function resolves to its implementation, not to the first
// signature, in the callees lookup and a crawl over it. Found by comparing
// with Jelly on solid-router (createAsync).
import { callGraph, crawl } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default async function transform(root) {
  const { callees } = callGraph("typescript");
  const main = root.root().find({ rule: { kind: "function_declaration" } });
  const all = await crawl(callees, main);
  expectEqual(
    [...all]
      .filter(([, edge]) => edge)
      .map(([node]) => node.field("name").text() + ":" + node.text().includes("return value")),
    ["pick:true", "useAmbient:false"],
    "callees of main, crawled",
  );
  // A `declare function` has no body in the workspace: ambient() has no edge.
  const useAmbient = [...all.keys()].find((node) => node.field("name")?.text() === "useAmbient");
  expectEqual(callees.has(useAmbient), false, "callees of useAmbient");
  return null;
}

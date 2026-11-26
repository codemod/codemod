// An overloaded function resolves to its implementation, not to the first
// signature. Found by comparing with Jelly on solid-router (createAsync).
import { callGraph, crawl } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const crawled = (root) =>
  crawl(
    callGraph("typescript").callees,
    root.root().find({ rule: { kind: "function_declaration" } }),
  );

test("a call to an overloaded function reaches its implementation", async (root) => {
  equal(
    [...(await crawled(root))]
      .filter(([, edge]) => edge)
      .map(([node]) => node.field("name").text() + ":" + node.text().includes("return value")),
    ["pick:true", "useAmbient:false"],
  );
});

test("a declare function has no body, so no callees", async (root) => {
  const useAmbient = [...(await crawled(root)).keys()].find(
    (n) => n.field("name")?.text() === "useAmbient",
  );
  equal(callGraph("typescript").callees.has(useAmbient), false);
});

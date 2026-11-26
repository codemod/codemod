// Without a semantic provider: the symbols and types lookups have no edges,
// and the call-graph and data-flow lookups and mutations() throw, since an
// empty answer would read as "no callers" or "no sources".
import { callGraph, dataflow, symbols, types } from "codemod:ast-grep";
import { equal, test, throws } from "./spec.js";
export { run as default } from "./spec.js";

const x = (root) => root.root().find({ rule: { pattern: "x" } });

test("definition has no edge", (root) => {
  equal(symbols("javascript").definition.get(x(root)), undefined);
});

test("references has no edge", (root) => {
  equal(symbols("javascript").references.get(x(root)), undefined);
});

test("typeOf has no type", (root) => {
  equal(types("javascript").typeOf.get(x(root)), undefined);
});

const queries = {
  "sources.get(node)": (node) => dataflow("javascript").sources.get(node),
  "uses.get(node)": (node) => dataflow("javascript").uses.get(node),
  "callees.get(node)": (node) => callGraph("javascript").callees.get(node),
  "callers.get(node)": (node) => callGraph("javascript").callers.get(node),
  "mutations()": (node) => node.mutations(),
};
for (const [query, ask] of Object.entries(queries)) {
  test(`${query} throws, saying it needs semantic analysis`, async (root) => {
    await throws(() => ask(x(root)), query + " needs semantic analysis");
  });
}

test("the error says which option to set", async (root) => {
  await throws(() => callGraph("javascript").callers.get(x(root)), 'semanticAnalysis: "file"');
});

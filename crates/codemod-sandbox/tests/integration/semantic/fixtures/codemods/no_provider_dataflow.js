// Without a semantic provider the call-graph and data-flow lookups and
// mutations() throw: an empty answer would read as "no callers" or
// "no sources".
import { callGraph, dataflow } from "codemod:ast-grep";

function expectThrow(run, needle, label) {
  try {
    run();
  } catch (error) {
    if (!String(error.message).includes(needle)) {
      throw new Error(label + ": unexpected message: " + error.message);
    }
    return;
  }
  throw new Error(label + ": expected an error");
}

export default function transform(root) {
  const node = root.root().find({ rule: { kind: "identifier" } });
  const { callers, callees } = callGraph("javascript");
  const { sources, uses } = dataflow("javascript");
  const queries = [
    ["sources.get(node)", () => sources.get(node)],
    ["uses.get(node)", () => uses.get(node)],
    ["callees.get(node)", () => callees.get(node)],
    ["callers.get(node)", () => callers.get(node)],
    ["mutations()", () => node.mutations()],
  ];
  for (const [query, run] of queries) {
    expectThrow(run, query + " needs semantic analysis", query);
  }
  expectThrow(() => callers.get(node), 'semanticAnalysis: "file"', "says what to set");
  return null;
}

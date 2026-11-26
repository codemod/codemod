// DRF class-based views and Celery tasks, in the callees lookup, and a
// request that a crawl of the uses lookup follows into a call.
import { callGraph, crawl, dataflow } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}
const basename = (p) => p.split("/").pop();
const where = (edge) =>
  edge.node.field("name").text() + "@" + basename(edge.node.getRoot().filename()) + ":" + edge.via;

export default async function transform(root) {
  const { callees } = callGraph("python");
  const { uses } = dataflow("python");
  const def = (name) =>
    root.root().find({ rule: { kind: "function_definition", has: { field: "name", regex: "^" + name + "$" } } });

  // Every call in list resolves: a constructor, a method on it, and a
  // method of the view.
  expectEqual(
    callees.get(def("list")).map(where).sort(),
    ["__init__@repo.py:constructor", "find@repo.py:method", "respond@api.py:method"],
    "callees of list",
  );
  // notify.delay(...) runs notify; the decorator is a call too.
  expectEqual(callees.get(def("respond")).map(where), ["notify@tasks.py:direct"], "callees of respond");
  // The request flows into repo.find(...) as an argument.
  const request = def("list").find({ rule: { kind: "identifier", regex: "^request$" } });
  expectEqual(
    [...(await crawl(uses, request)).values()].some(
      (edge) => edge?.via === "argument" && edge.name === "repo.find",
    ),
    true,
    "request reaches repo.find",
  );
  return null;
}

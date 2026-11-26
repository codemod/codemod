// The callers lookup through views, the callees lookup through annotations,
// list mutations, and the typeOf lookup for annotations, on a Django-ish app.
import { callGraph, types } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}
const basename = (p) => p.split("/").pop();
const where = (node) => node.field("name").text() + "@" + basename(node.getRoot().filename());

export default function transform(root) {
  const { callers, callees } = callGraph("python");
  const { typeOf } = types("python");
  const def = (name) =>
    root.root().find({ rule: { kind: "function_definition", has: { field: "name", regex: "^" + name + "$" } } });

  // callers: a service function, through the views that call it.
  expectEqual(
    callers.get(def("place_order").field("name")).map((e) => where(e.node) + " " + e.site.text()),
    ['checkout@views.py place_order(request.user, request.POST.getlist("items"))'],
    "callers of place_order",
  );
  // `from . import services` then `services.find_orders(...)`.
  expectEqual(
    callers.get(def("find_orders")).map((e) => where(e.node) + " " + e.via),
    ["history@views.py method"],
    "callers of find_orders",
  );

  // callees: `repo: OrderRepository` makes repo.find() resolve.
  expectEqual(callees.get(def("find_orders")).map((e) => where(e.node)), ["find@repo.py"], "callees of find_orders");

  // A call the analysis can't resolve (len) has no edge.
  const toLen = (callees.get(def("place_order")) ?? []).filter((e) => e.site.text().startsWith("len("));
  expectEqual(toLen.length, 0, "len has no callee");

  // mutations(): a list.
  const lines = root.root().find({ rule: { pattern: "lines = []" } }).field("left");
  expectEqual(
    lines.mutations().map((m) => m.kind + ":" + m.confidence + " " + m.node.text()),
    ["method-call:likely lines.append(item)", "method-call:likely lines.sort()", "escape:maybe len(lines)"],
    "mutations of lines",
  );

  // typeOf: annotations only, as written.
  const repo = def("find_orders").find({ rule: { pattern: "repo.find(customer)" } }).field("function").field("object");
  expectEqual(typeOf.get(repo), "OrderRepository", "type of repo");

  return null;
}

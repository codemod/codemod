// Callers through views, callees through annotations, list mutations and
// annotation types, on a Django-ish app.
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}
const basename = (p) => p.split("/").pop();

export default function transform(root) {
  const def = (name) =>
    root.root().find({
      rule: { kind: "function_definition", has: { field: "name", regex: "^" + name + "$" } },
    });

  // callers(): a service function, through the views that call it.
  expectEqual(
    def("place_order")
      .field("name")
      .callers()
      .map((c) => c.name + "@" + basename(c.root.filename()) + " " + c.callSite.node.text()),
    ['checkout@views.py place_order(request.user, request.POST.getlist("items"))'],
    "callers of place_order",
  );
  // `from . import services` then `services.find_orders(...)`.
  expectEqual(
    def("find_orders")
      .callers()
      .map((c) => c.name + "@" + basename(c.root.filename()) + " " + c.via),
    ["history@views.py method"],
    "callers of find_orders",
  );

  // callees(): `repo: OrderRepository` makes repo.find() resolve.
  const callees = def("find_orders")
    .callees()
    .map((c) => (c.resolved ? "" : "?") + c.name + "@" + basename(c.root.filename()));
  expectEqual(callees, ["find@repo.py"], "callees of find_orders");

  // Unresolved calls say so.
  const placeCallees = def("place_order")
    .callees()
    .filter((c) => !c.resolved)
    .map((c) => c.name);
  expectEqual(placeCallees.includes("len"), true, "len is unresolved");

  // mutations(): a list.
  const lines = root
    .root()
    .find({ rule: { pattern: "lines = []" } })
    .field("left");
  expectEqual(
    lines.mutations().map((m) => m.kind + ":" + m.confidence + " " + m.node.text()),
    [
      "method-call:likely lines.append(item)",
      "method-call:likely lines.sort()",
      "escape:maybe len(lines)",
    ],
    "mutations of lines",
  );

  // typeInfo(): annotations only.
  const repo = def("find_orders")
    .find({ rule: { pattern: "repo.find(customer)" } })
    .field("function")
    .field("object");
  expectEqual(repo.typeInfo(), "OrderRepository", "type of repo");

  return null;
}

// FastAPI + SQLAlchemy-ish: taint into text(f"..."), method resolution
// through annotated fields, return annotations and package re-exports,
// decorators as calls.
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
    root.root().find({ rule: { kind: "function_definition", has: { field: "name", regex: "^" + name + "$" } } });

  // The query parameter reaches the SQL text through repo.search (an
  // annotated receiver) and into text(f"...").
  const q = def("list_orders").find({ rule: { pattern: "repo.search(q)" } });
  const callees = q.callees().map((c) => c.name + "@" + basename(c.root.filename()));
  expectEqual(callees, ["search@repository.py"], "repo.search resolves");

  // create_order: get_repo() -> OrderRepository, so repo.save resolves,
  // and save calls self.validate.
  const created = def("create_order")
    .callees({ transitive: true })
    .filter((c) => c.resolved)
    .map((c) => c.name + "@" + basename(c.root.filename()))
    .sort();
  expectEqual(
    created,
    ["__init__@repository.py", "add@db.py", "get_repo@main.py", "save@repository.py", "validate@repository.py"],
    "callees of create_order",
  );

  // Decorators are calls: the module calls app.get(...) with list_orders.
  const viaDecorator = def("list_orders")
    .callers()
    .map((c) => c.name + " " + c.via);
  expectEqual(viaDecorator, ["<module> callback"], "list_orders is handed to app.get");

  // Comprehension: tags come from payload.
  const tags = def("create_order").find({ rule: { pattern: "tags = $V" } }).field("left");
  expectEqual(
    tags.dependsOn((s) => s.kind === "parameter" && s.name === "payload"),
    true,
    "tags depend on payload",
  );

  // typeInfo(): parameters, returns, instances.
  expectEqual(
    def("list_orders").find({ rule: { pattern: "repo.search(q)" } }).field("arguments").find({ rule: { kind: "identifier", regex: "^q$" } }).typeInfo(),
    "str",
    "q is str",
  );
  expectEqual(
    def("create_order").find({ rule: { pattern: "repo = $V" } }).field("left").typeInfo(),
    "OrderRepository",
    "get_repo() is an OrderRepository",
  );
  return null;
}

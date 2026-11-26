// FastAPI + SQLAlchemy-ish, through the callGraph, dataflow and types
// lookups: method resolution through annotated fields, return annotations
// and package re-exports, decorators as calls, taint through a
// comprehension, and the types of parameters and assigned names.
import { callGraph, crawl, dataflow, types } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}
const basename = (p) => p.split("/").pop();
// A function's name. A caller outside any function is the module's
// top-level code; its edge's node is the call itself.
const nameOf = (node) =>
  node.kind() === "function_definition" ? node.field("name").text() : "<module>";
const where = (node) => nameOf(node) + "@" + basename(node.getRoot().filename());

export default async function transform(root) {
  const { callees, callers } = callGraph("python");
  const { sources } = dataflow("python");
  const { typeOf } = types("python");
  const def = (name) =>
    root.root().find({ rule: { kind: "function_definition", has: { field: "name", regex: "^" + name + "$" } } });

  // repo.search(q) resolves through the annotated receiver.
  const q = def("list_orders").find({ rule: { pattern: "repo.search(q)" } });
  expectEqual(callees.get(q).map((e) => where(e.node)), ["search@repository.py"], "repo.search resolves");

  // create_order: get_repo() -> OrderRepository, so repo.save resolves,
  // and save calls self.validate. The crawl reaches every function called.
  const created = [...(await crawl(callees, def("create_order")))]
    .filter(([, edge]) => edge)
    .map(([node]) => where(node))
    .sort();
  expectEqual(
    created,
    ["__init__@repository.py", "add@db.py", "get_repo@main.py", "save@repository.py", "validate@repository.py"],
    "callees of create_order",
  );

  // Decorators are calls: the module calls app.get(...) with list_orders,
  // at the decorator.
  const viaDecorator = callers
    .get(def("list_orders"))
    .map((e) => nameOf(e.node) + " " + e.via + " " + e.site.kind());
  expectEqual(viaDecorator, ["<module> callback decorator"], "list_orders is handed to app.get");

  // Comprehension: tags come from payload.
  const tags = def("create_order").find({ rule: { pattern: "tags = $V" } }).field("left");
  expectEqual(
    [...(await crawl(sources, tags)).values()].some(
      (edge) => edge?.kind === "parameter" && edge.name === "payload",
    ),
    true,
    "tags depend on payload",
  );

  // typeOf: parameters, returns, instances.
  expectEqual(
    typeOf.get(
      def("list_orders")
        .find({ rule: { pattern: "repo.search(q)" } })
        .field("arguments")
        .find({ rule: { kind: "identifier", regex: "^q$" } }),
    ) ?? null,
    "str",
    "q is str",
  );
  expectEqual(
    typeOf.get(def("create_order").find({ rule: { pattern: "repo = $V" } }).field("left")) ?? null,
    "OrderRepository",
    "get_repo() is an OrderRepository",
  );
  return null;
}

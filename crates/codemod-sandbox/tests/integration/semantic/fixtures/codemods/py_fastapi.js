// FastAPI + SQLAlchemy-ish: method resolution through annotated fields,
// return annotations and package re-exports, decorators as calls, taint
// through a comprehension and into SQL text, and the types of parameters
// and assigned names.
import { callGraph, crawl, dataflow, types } from "codemod:ast-grep";
import { equal, onFile, test } from "./spec.js";
export { run as default } from "./spec.js";

const basename = (p) => p.split("/").pop();
// A function's name; a caller outside any function is the module's top
// level, and its edge's node is the call itself.
const nameOf = (node) =>
  node.kind() === "function_definition" ? node.field("name").text() : "<module>";
const where = (node) => nameOf(node) + "@" + basename(node.getRoot().filename());
const def = (root, name) =>
  root.root().find({
    rule: { kind: "function_definition", has: { field: "name", regex: "^" + name + "$" } },
  });
// Whether a crawl from `node` reaches an edge `test` accepts.
const reaches = async (lookup, node, test) =>
  [...(await crawl(lookup, node)).values()].some((e) => e && test(e));

onFile("app/main.py", () => {
  test("a method call on an annotated parameter resolves", (root) => {
    const call = def(root, "list_orders").find({ rule: { pattern: "repo.search(q)" } });
    equal(
      callGraph("python")
        .callees.get(call)
        .map((e) => where(e.node)),
      ["search@repository.py"],
    );
  });

  test("a crawl of callees follows return annotations and self methods", async (root) => {
    const created = [...(await crawl(callGraph("python").callees, def(root, "create_order")))]
      .filter(([, edge]) => edge)
      .map(([node]) => where(node))
      .sort();
    equal(created, [
      "__init__@repository.py",
      "add@db.py",
      "get_repo@main.py",
      "save@repository.py",
      "validate@repository.py",
    ]);
  });

  test("a decorator is a call handing the function to it", (root) => {
    equal(
      callGraph("python")
        .callers.get(def(root, "list_orders"))
        .map((e) => nameOf(e.node) + " " + e.via + " " + e.site.kind()),
      ["<module> callback decorator"],
    );
  });

  test("a comprehension's result comes from what it iterates", async (root) => {
    const tags = def(root, "create_order")
      .find({ rule: { pattern: "tags = $V" } })
      .field("left");
    equal(
      await reaches(
        dataflow("python").sources,
        tags,
        (e) => e.kind === "parameter" && e.name === "payload",
      ),
      true,
    );
  });

  test("typeOf of a parameter is its annotation", (root) => {
    const q = def(root, "list_orders")
      .find({ rule: { pattern: "repo.search(q)" } })
      .field("arguments")
      .find({ rule: { kind: "identifier", regex: "^q$" } });
    equal(types("python").typeOf.get(q) ?? null, "str");
  });

  test("typeOf of a name assigned a call is the callee's return annotation", (root) => {
    const repo = def(root, "create_order")
      .find({ rule: { pattern: "repo = $V" } })
      .field("left");
    equal(types("python").typeOf.get(repo) ?? null, "OrderRepository");
  });
});

onFile("app/repository.py", () => {
  test("a parameter flows into SQL text", async (root) => {
    const sql = root
      .root()
      .find({ rule: { pattern: "text($SQL)" } })
      .getMatch("SQL");
    equal(
      await reaches(
        dataflow("python").sources,
        sql,
        (e) => e.kind === "parameter" && e.name === "term",
      ),
      true,
    );
  });

  test("a parameter flows on into a method call on an annotated attribute", async (root) => {
    const term = root.root().find({ rule: { kind: "identifier", regex: "^term$" } });
    equal(
      await reaches(
        dataflow("python").uses,
        term,
        (e) => e.via === "argument" && e.name === "self.session.execute",
      ),
      true,
    );
  });

  test("a method on an annotated class attribute resolves through a package re-export", (root) => {
    const execute = root.root().find({ rule: { pattern: "self.session.execute($$$)" } });
    equal(
      (callGraph("python").callees.get(execute) ?? []).map((e) => e.node.field("name").text()),
      ["execute"],
    );
  });
});

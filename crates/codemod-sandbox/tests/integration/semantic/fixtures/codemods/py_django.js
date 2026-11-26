// A Django-ish app: callers through views, callees through annotations, DRF
// class-based views and Celery tasks, list mutations, and the typeOf lookup
// for annotations.
import { callGraph, crawl, dataflow, types } from "codemod:ast-grep";
import { equal, onFile, test } from "./spec.js";
export { run as default } from "./spec.js";

const basename = (p) => p.split("/").pop();
const where = (node) => node.field("name").text() + "@" + basename(node.getRoot().filename());
const def = (root, name) =>
  root.root().find({
    rule: { kind: "function_definition", has: { field: "name", regex: "^" + name + "$" } },
  });

onFile("shop/services.py", () => {
  test("callers of a service function are the views calling it", (root) => {
    equal(
      callGraph("python")
        .callers.get(def(root, "place_order").field("name"))
        .map((e) => where(e.node) + " " + e.site.text()),
      ['checkout@views.py place_order(request.user, request.POST.getlist("items"))'],
    );
  });

  test("a call through an imported module is a method call", (root) => {
    equal(
      callGraph("python")
        .callers.get(def(root, "find_orders"))
        .map((e) => where(e.node) + " " + e.via),
      ["history@views.py method"],
    );
  });

  test("an annotated parameter's method call resolves", (root) => {
    equal(
      callGraph("python")
        .callees.get(def(root, "find_orders"))
        .map((e) => where(e.node)),
      ["find@repo.py"],
    );
  });

  test("a built-in call has no callee", (root) => {
    const callees = callGraph("python").callees.get(def(root, "place_order")) ?? [];
    equal(
      callees.filter((e) => e.site.text().startsWith("len(")),
      [],
    );
  });

  test("mutations of a list include its method calls and escapes", (root) => {
    const lines = root
      .root()
      .find({ rule: { pattern: "lines = []" } })
      .field("left");
    equal(
      lines.mutations().map((m) => m.kind + ":" + m.confidence + " " + m.node.text()),
      [
        "method-call:likely lines.append(item)",
        "method-call:likely lines.sort()",
        "escape:maybe len(lines)",
      ],
    );
  });

  test("typeOf of an annotated parameter is the annotation, as written", (root) => {
    const repo = def(root, "find_orders")
      .find({ rule: { pattern: "repo.find(customer)" } })
      .field("function")
      .field("object");
    equal(types("python").typeOf.get(repo), "OrderRepository");
  });
});

onFile("shop/api.py", () => {
  const callees = (node) =>
    callGraph("python")
      .callees.get(node)
      .map((e) => where(e.node) + ":" + e.via);

  test("a view method's callees are a constructor, a method on it and the view's own method", (root) => {
    equal(callees(def(root, "list")).sort(), [
      "__init__@repo.py:constructor",
      "find@repo.py:method",
      "respond@api.py:method",
    ]);
  });

  test("task.delay(...) calls the Celery task", (root) => {
    equal(callees(def(root, "respond")), ["notify@tasks.py:direct"]);
  });

  test("the request flows into a method call as an argument", async (root) => {
    const request = def(root, "list").find({ rule: { kind: "identifier", regex: "^request$" } });
    const reached = await crawl(dataflow("python").uses, request);
    equal(
      [...reached.values()].some((e) => e?.via === "argument" && e.name === "repo.find"),
      true,
    );
  });
});

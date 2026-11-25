// DRF class-based views and Celery tasks.
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

  expectEqual(
    def("list")
      .callees()
      .map((c) => (c.resolved ? "" : "?") + c.name + "@" + basename(c.root.filename()) + ":" + c.via)
      .sort(),
    ["__init__@repo.py:constructor", "find@repo.py:method", "respond@api.py:method"],
    "callees of list",
  );
  // notify.delay(...) runs notify; the decorator is a call too.
  expectEqual(
    def("respond")
      .callees()
      .map((c) => c.name + "@" + basename(c.root.filename()) + ":" + c.via),
    ["notify@tasks.py:direct"],
    "callees of respond",
  );
  // The request flows into repo.find(...) as an argument.
  const request = def("list").find({ rule: { kind: "identifier", regex: "^request$" } });
  expectEqual(
    request.reaches((s) => s.via === "argument" && s.name === "repo.find"),
    true,
    "request reaches repo.find",
  );
  return null;
}

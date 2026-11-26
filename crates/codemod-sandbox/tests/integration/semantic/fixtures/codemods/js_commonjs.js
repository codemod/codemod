// CommonJS in the call graph: require(), module.exports, exports.x,
// re-exports, and ES modules importing CommonJS.
import { callGraph, crawl } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

// A function's name: its own, or the name it is bound to.
function nameOf(fn) {
  const own = fn.field("name");
  if (own) return own.text();
  const parent = fn.parent();
  const bound = parent && (parent.field("name") || parent.field("key"));
  return bound ? bound.text() : "<anonymous>";
}
const where = (node) => nameOf(node) + "@" + node.getRoot().filename().split("/").pop();
const run = (root) => root.root().find({ rule: { kind: "function_declaration" } });
const callee = (root, name) =>
  callGraph("javascript")
    .callees.get(run(root))
    .find((e) => nameOf(e.node) === name).node;

// A package outside the workspace (pad) and a dynamic require
// (plugin.init) have no edge.
test("callees follow require, module.exports and exports.x", (root) => {
  equal(
    callGraph("javascript")
      .callees.get(run(root))
      .map((e) => where(e.node))
      .sort(),
    [
      "constructor@db.js", // new Db(), module.exports = Db
      "format@lib.js", // lib.format
      "format@lib.js", // again.format through module.exports = require("./lib")
      "load@store.js", // module.exports = { load: () => null }
      "parse@lib.js", // const { parse } = require("./lib"); module.exports.parse = parse
      "query@db.js", // db.query() on new Db()
      "save@store.js", // store.save()
      "save@store.js", // persist: exports.persist = require("./store").save
    ],
  );
});

test("a crawl goes on from a CommonJS export to what it calls", async (root) => {
  const all = await crawl(callGraph("javascript").callees, run(root));
  const helper = [...all.keys()].find((node) => nameOf(node) === "helper");
  const format = all.get(helper)?.from;
  equal([nameOf(format), all.get(format).from.equals(run(root))], ["format", true]);
});

test("a re-exported function's call sites are all callers of it", (root) => {
  equal(
    callGraph("javascript")
      .callers.get(callee(root, "save"))
      .map((e) => e.site.text())
      .sort(),
    ["persist(input)", "store.save(input)"],
  );
});

test("an ES module importing a CommonJS export is among its callers", (root) => {
  equal(
    callGraph("javascript")
      .callers.get(callee(root, "format"))
      .map((e) => where(e.node) + " " + e.site.text())
      .sort(),
    [
      "render@esm.mjs format(value)",
      "run@main.js again.format(input)",
      "run@main.js lib.format(input)",
    ],
  );
  equal(
    callGraph("javascript")
      .callers.get(callee(root, "constructor"))
      .map((e) => where(e.node))
      .sort(),
    ["render@esm.mjs", "run@main.js"],
    "the Db constructor",
  );
});

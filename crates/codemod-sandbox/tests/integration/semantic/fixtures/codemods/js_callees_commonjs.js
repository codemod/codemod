// CommonJS in the call graph: require(), module.exports, exports.x,
// re-exports, and ESM importing CommonJS, through the callees and callers
// lookups.
import { callGraph, crawl } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

function basename(path) {
  return path.split("/").pop();
}

// A function's name: its own, or the name it is bound to.
function nameOf(fn) {
  const own = fn.field("name");
  if (own) return own.text();
  const parent = fn.parent();
  const bound = parent && (parent.field("name") || parent.field("key"));
  return bound ? bound.text() : "<anonymous>";
}

const where = (node) => nameOf(node) + "@" + basename(node.getRoot().filename());

export default async function transform(root) {
  const { callees, callers } = callGraph("javascript");
  const run = root.root().find({ rule: { kind: "function_declaration" } });
  if (!run) throw new Error("run not found");

  // A package outside the workspace (pad) and a dynamic require
  // (plugin.init) have no edge.
  const direct = callees.get(run);
  expectEqual(
    direct.map((e) => where(e.node)).sort(),
    [
      "constructor@db.js", // new Db(), module.exports = Db
      "format@lib.js", // lib.format
      "format@lib.js", // again.format through module.exports = require("./lib"): its own call site
      "load@store.js", // module.exports = { load: () => null }
      "parse@lib.js", // const { parse } = require("./lib"); module.exports.parse = parse
      "query@db.js", // db.query() on new Db()
      "save@store.js", // store.save()
      "save@store.js", // persist: exports.persist = require("./store").save
    ],
    "callees of run",
  );

  // format calls helper: one hop further.
  const all = await crawl(callees, run);
  const helper = [...all.keys()].find((node) => nameOf(node) === "helper");
  if (!helper || nameOf(all.get(helper).from) !== "format" || all.get(all.get(helper).from).from !== run) {
    throw new Error("format calls helper: " + JSON.stringify([...all.keys()].map(nameOf)));
  }

  const find = (name) => direct.find((e) => nameOf(e.node) === name).node;

  // Both ways of reaching save are call sites of it.
  expectEqual(
    callers
      .get(find("save"))
      .map((e) => e.site.text())
      .sort(),
    ["persist(input)", "store.save(input)"],
    "callers of save",
  );

  // `import { format } from "./lib.js"` in an ES module reaches the
  // CommonJS export too.
  expectEqual(
    callers
      .get(find("format"))
      .map((e) => where(e.node) + " " + e.site.text())
      .sort(),
    [
      "render@esm.mjs format(value)",
      "run@main.js again.format(input)",
      "run@main.js lib.format(input)",
    ],
    "callers of format",
  );
  expectEqual(
    callers
      .get(find("constructor"))
      .map((e) => where(e.node))
      .sort(),
    ["render@esm.mjs", "run@main.js"],
    "callers of the Db constructor",
  );

  return null;
}

// CommonJS: require(), module.exports, exports.x, re-exports, and ESM
// importing CommonJS.
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

export default function transform(root) {
  const run = root.root().find({ rule: { kind: "function_declaration" } });
  if (!run) throw new Error("run not found");

  const direct = run.callees();
  expectEqual(
    direct
      .filter((c) => c.resolved)
      .map((c) => c.name + "@" + basename(c.root.filename()))
      .sort(),
    [
      "constructor@db.js", // new Db(), module.exports = Db
      "format@lib.js", // lib.format, and again.format through module.exports = require("./lib")
      "load@store.js", // module.exports = { load: () => null }
      "parse@lib.js", // const { parse } = require("./lib"); module.exports.parse = parse
      "query@db.js", // db.query() on new Db()
      "save@store.js", // store.save(), and persist: exports.persist = require("./store").save
    ],
    "callees of run",
  );
  // Can't tell: a package outside the workspace, and a dynamic require.
  expectEqual(
    direct
      .filter((c) => !c.resolved)
      .map((c) => c.name)
      .sort(),
    ["pad", "plugin.init"],
    "unresolved callees of run",
  );

  const transitive = run.callees({ transitive: true }).filter((c) => c.resolved);
  if (!transitive.some((c) => c.name === "helper" && c.depth === 2)) {
    throw new Error("format calls helper: " + JSON.stringify(transitive.map((c) => c.name)));
  }

  const find = (name) => direct.find((c) => c.name === name).node;

  // Both ways of reaching save are call sites of it.
  expectEqual(
    find("save")
      .callers()
      .map((c) => c.callSite.node.text())
      .sort(),
    ["persist(input)", "store.save(input)"],
    "callers of save",
  );

  // `import { format } from "./lib.js"` in an ES module reaches the
  // CommonJS export too.
  expectEqual(
    find("format")
      .callers()
      .map((c) => c.name + "@" + basename(c.root.filename()) + " " + c.callSite.node.text())
      .sort(),
    [
      "render@esm.mjs format(value)",
      "run@main.js again.format(input)",
      "run@main.js lib.format(input)",
    ],
    "callers of format",
  );
  expectEqual(
    find("constructor")
      .callers()
      .map((c) => c.name + "@" + basename(c.root.filename()))
      .sort(),
    ["render@esm.mjs", "run@main.js"],
    "callers of the Db constructor",
  );

  return null;
}

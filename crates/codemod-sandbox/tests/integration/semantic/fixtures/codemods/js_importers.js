// root.importers(): every place that refers to a module, by kind.
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}

export default function transform(root) {
  const found = root.importers();
  expectEqual(
    found
      .map(
        (i) =>
          i.root.filename().split("/").pop() +
          " " +
          i.kind +
          " " +
          i.specifier +
          " | " +
          i.node.text().split("\n")[0],
      )
      .sort(),
    [
      'app.js import ./lib | import { helper } from "./lib";',
      'app.test.js mock ./lib | jest.mock("./lib")',
      'app.test.js mock ./lib | jest.requireActual("./lib")',
      'barrel.js export ./lib | export * from "./lib";',
      'guide.mdx import ../src/lib | import { helper } from "../src/lib";',
      'lazy.js dynamic ./lib | import("./lib")',
      'legacy.js require ./lib | require("./lib")',
      'setup.js import ./lib/index.js | import "./lib/index.js";',
    ],
    "importers of lib/index.js",
  );
  expectEqual(found.unparsed, [], "complete");

  // A snapshot: a file rewritten during the run still counts as importing
  // what it imported when the table was built (as for callers()).
  found.find((i) => i.kind === "export").root.write('export { other } from "./other";\n');
  expectEqual(root.importers().length, found.length, "same answer after an importer was rewritten");

  // Another file, by path relative to this one.
  expectEqual(
    root
      .importers("../other.js")
      .map((i) => i.root.filename().split("/").pop() + " " + i.kind)
      .sort(),
    ["app.js import", "app.test.js mock", "barrel.js export"],
    "importers of other.js",
  );
  return null;
}

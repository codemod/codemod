// The importers lookup: every place that refers to a module, by kind.
import { symbols } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

// Each edge as "file kind specifier | the site's first line", sorted.
const importers = (root) =>
  (symbols("javascript").importers.get(root.root()) ?? [])
    .map(
      (e) =>
        e.node.getRoot().filename().split("/").pop() +
        " " +
        e.kind +
        " " +
        e.specifier +
        " | " +
        e.site.text().split("\n")[0],
    )
    .sort();

test("importers of a module are its imports, re-exports, requires, dynamic imports and mocks", (root) => {
  equal(importers(root), [
    'app.js import ./lib | import { helper } from "./lib";',
    'app.test.js mock ./lib | jest.mock("./lib")',
    'app.test.js mock ./lib | jest.requireActual("./lib")',
    'barrel.js export ./lib | export * from "./lib";',
    'guide.mdx import ../src/lib | import { helper } from "../src/lib";',
    'lazy.js dynamic ./lib | import("./lib")',
    'legacy.js require ./lib | require("./lib")',
    'setup.js import ./lib/index.js | import "./lib/index.js";',
  ]);
});

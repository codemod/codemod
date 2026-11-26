// specifierFor and publicSpecifiers across workspace packages: only what the
// target package makes public (its exports map, or its bare name without
// one), never a relative path into its files.
import { symbols } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const { resolveModule, specifierFor, references } = symbols("typescript");
const leaf = (root, module, name) => root.resolveExport(module, name).root;
const like = { like: "@acme/ui" };

test("specifierFor uses an exports subpath with conditions", (root) => {
  equal(specifierFor(root, leaf(root, "@acme/ui", "Button"), like), "@acme/ui/button");
});

test("specifierFor uses an exports pattern", (root) => {
  equal(specifierFor(root, leaf(root, "@acme/ui", "Check"), like), "@acme/ui/icons/Check");
});

test("specifierFor answers null for a file the exports map leaves out", (root) => {
  equal(specifierFor(root, leaf(root, "@acme/ui", "Modal"), like), null);
});

test("specifierFor a package entry is the package name", (root) => {
  equal(specifierFor(root, resolveModule(root, "@acme/ui"), like), "@acme/ui");
  equal(specifierFor(root, resolveModule(root, "@acme/lib")), "@acme/lib", "without exports");
});

test("without an exports map, deep files are private", (root) => {
  equal(specifierFor(root, leaf(root, "@acme/lib", "format"), { like: "@acme/lib" }), null);
  equal(leaf(root, "@acme/lib", "format").publicSpecifiers(), []);
});

test("publicSpecifiers of a file are the exports subpaths that reach it", (root) => {
  equal(
    [
      leaf(root, "@acme/ui", "Button").publicSpecifiers(),
      leaf(root, "@acme/ui", "Modal").publicSpecifiers(),
    ],
    [["@acme/ui/button"], []],
  );
});

test("publicSpecifiers of an app file are empty", (root) => {
  equal(root.publicSpecifiers(), []);
});

test("publicSpecifiers of a package entry is the package name", (root) => {
  const button = leaf(root, "@acme/ui", "Button");
  const entry = references
    .get(button.root().find({ rule: { kind: "identifier", regex: "^Button$" } }))
    .map((e) => e.node.getRoot())
    .find((r) => r.filename().endsWith("ui/src/index.ts"));
  equal(entry.publicSpecifiers(), ["@acme/ui"]);
});

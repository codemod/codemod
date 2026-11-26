// symbols(lang).resolveModule and .specifierFor, and root.resolveExport,
// with tsconfig `paths` inherited through `extends`.
import { symbols } from "codemod:ast-grep";
import { equal, test, throws } from "./spec.js";
export { run as default } from "./spec.js";

const tail = (path) => path && path.split("/").slice(-3).join("/");
const { resolveModule, specifierFor } = symbols("typescript");
const button = (root) => root.resolveExport(resolveModule(root, "@/ui"), "Button");
const modal = (root) => root.resolveExport("@/ui", "Modal");

test("resolveModule follows an alias with *, an exact alias and a relative path", (root) => {
  equal(
    ["@/ui", "~ui", "../ui/button"].map((specifier) => tail(resolveModule(root, specifier))),
    ["src/ui/index.ts", "src/ui/index.ts", "src/ui/button.ts"],
  );
});

test("resolveModule answers null for a specifier that does not resolve", (root) => {
  equal(resolveModule(root, "@/nope"), null);
});

test("resolveExport goes through a barrel to the declaration, with how it is exported", (root) => {
  const m = modal(root);
  equal(
    [tail(m.root.filename()), m.kind, m.exportName, m.nameNode.text()],
    ["src/ui/modal.ts", "external", "default", "Modal"],
  );
});

test("resolveExport answers null for a name the module does not export", (root) => {
  equal(root.resolveExport("@/ui", "Missing"), null);
});

test("resolveExport takes a path as well as a specifier", (root) => {
  equal(button(root).exportName, "Button");
});

test("specifierFor keeps the style of the specifier it replaces", (root) => {
  equal(
    [
      specifierFor(root, modal(root).root, { like: "@/ui" }),
      specifierFor(root, button(root).root, { like: "../ui" }),
      specifierFor(root, button(root).root, { like: "../ui/index.js" }),
    ],
    ["@/ui/modal", "../ui/button", "../ui/button.js"],
  );
});

test("specifierFor prefers an alias when asked", (root) => {
  equal(specifierFor(root, button(root).root, { prefer: "alias" }), "@/ui/button");
});

test("specifierFor an index file drops the index, shortest first", (root) => {
  const index = resolveModule(root, "@/ui");
  equal([specifierFor(root, index, { like: "@/ui" }), specifierFor(root, index)], ["@/ui", "~ui"]);
});

test("specifierFor answers null for a file that does not exist", (root) => {
  equal(specifierFor(root, resolveModule(root, "@/ui").replace(/index\.ts$/, "missing.ts")), null);
});

test("specifierFor rejects an unknown prefer", async (root) => {
  await throws(
    () => specifierFor(root, resolveModule(root, "@/ui"), { prefer: "short" }),
    'prefer must be "relative" or "alias"',
  );
});

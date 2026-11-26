// What importing a module runs: the callees lookup on a program node
// answers with the calls made at its top level, and root.hasTopLevelEffects()
// says whether it runs code at all. Fixture: debarrel gap D (side-effect
// barrel).
import { callGraph, crawl, symbols } from "codemod:ast-grep";
import { equal, onFile, test } from "./spec.js";
export { run as default } from "./spec.js";

const callees = (node) =>
  (callGraph("typescript").callees.get(node) ?? []).map(
    (e) => e.via + ":" + e.node.field("name").text(),
  );

onFile("src/icons/register.ts", () => {
  test("a top-level call to a Map method has no callee", (root) => {
    equal(callees(root.root()), []);
  });
});

onFile("src/icons/Icon.ts", () => {
  test("a module that only declares a function calls nothing on import", (root) => {
    equal(callees(root.root()), []);
  });

  test("a function calling only a Map method has no callees", (root) => {
    equal(callees(root.root().find({ rule: { kind: "function_declaration" } })), []);
  });
});

onFile("src/App.ts", () => {
  test("callees of a program are the workspace functions its top level calls", (root) => {
    equal(callees(root.root()), ["direct:Icon"]);
  });

  test("a crawl from a program stops where calls leave the workspace", async (root) => {
    const reached = await crawl(callGraph("typescript").callees, root.root());
    equal(
      [...reached]
        .filter(([, edge]) => edge)
        .map(([node, edge]) => edge.via + ":" + node.field("name").text()),
      ["direct:Icon"],
    );
  });

  // Roots for the other files, reached from App.ts.
  const rootReferring = (root, name, file) =>
    symbols("typescript")
      .references.get(root.root().find({ rule: { kind: "identifier", regex: `^${name}$` } }))
      .map((e) => e.node.getRoot())
      .find((r) => r.filename().endsWith(file));

  test("hasTopLevelEffects: true for a top-level call", (root) => {
    equal(root.hasTopLevelEffects(), true);
  });

  test("hasTopLevelEffects: false for a module that only declares", (root) => {
    equal(root.resolveExport("./icons", "Icon").root.hasTopLevelEffects(), false);
  });

  test("hasTopLevelEffects: true for a bare import", (root) => {
    const icon = root.resolveExport("./icons", "Icon").root;
    equal(rootReferring(icon, "Icon", "icons/index.ts").hasTopLevelEffects(), true);
  });

  test("hasTopLevelEffects: true for a method call on an imported value", (root) => {
    const registry = root.resolveExport("./registry", "registry").root;
    equal(rootReferring(registry, "registry", "register.ts").hasTopLevelEffects(), true);
  });
});

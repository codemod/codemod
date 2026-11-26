// A workspace package imported by name, with nothing installed and its dist/
// not built: lookups still land in its sources. Fixture: debarrel gap G
// (library entry).
import { callGraph, symbols } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const basename = (path) => path.split("/").pop();

// @acme/ui's main is dist/index.js, which does not exist, so src/index.ts,
// through its re-export; @acme/ui/card is src/card.ts; not-installed is no
// workspace package, so thing() has no edge.
test("callees of a program land in an unbuilt workspace package's sources", (root) => {
  equal(
    callGraph("typescript")
      .callees.get(root.root())
      .map((e) => e.node.field("name").text() + "@" + basename(e.node.getRoot().filename())),
    ["Button@Button.ts", "Card@card.ts"],
  );
});

test("definition of an import from an unbuilt workspace package lands in its sources", (root) => {
  const button = root.root().find({ rule: { kind: "identifier", regex: "^Button$" } });
  const [def] = symbols("typescript").definition.get(button);
  equal(basename(def.node.getRoot().filename()), "Button.ts");
});

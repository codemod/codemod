// A workspace package imported by name, with nothing installed and its
// dist/ not built: the callees lookup on the program and the definition
// lookup still land in its sources. Fixture: debarrel gap G (library entry).
import { callGraph, symbols } from "codemod:ast-grep";

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
  const { callees } = callGraph("typescript");
  const { definition } = symbols("typescript");
  const calls = callees
    .get(root.root())
    .map((e) => e.node.field("name").text() + "@" + basename(e.node.getRoot().filename()));
  // @acme/ui: package.json main is dist/index.js, which does not exist, so
  // src/index.ts, through its re-export. @acme/ui/card: src/card.ts.
  // not-installed is not a workspace package: thing() has no edge.
  expectEqual(calls, ["Button@Button.ts", "Card@card.ts"], "module callees");

  const button = root.root().find({ rule: { kind: "identifier", regex: "^Button$" } });
  const def = definition.get(button)?.[0];
  if (!def || basename(def.node.getRoot().filename()) !== "Button.ts") {
    throw new Error(
      "definition.get(node) of the import should land in Button.ts, got " +
        (def ? def.node.getRoot().filename() : "undefined"),
    );
  }
  return null;
}

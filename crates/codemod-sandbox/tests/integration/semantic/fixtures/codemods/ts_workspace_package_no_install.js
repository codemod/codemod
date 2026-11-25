// A workspace package imported by name, with nothing installed and its
// dist/ not built. Fixture: debarrel gap G (library entry).
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
  const calls = root
    .root()
    .callees()
    .map((c) => (c.resolved ? c.name + "@" + basename(c.root.filename()) : "u:" + c.name));
  // @acme/ui: package.json main is dist/index.js, which does not exist, so
  // src/index.ts, through its re-export. @acme/ui/card: src/card.ts.
  // not-installed: not a workspace package; reported unresolved.
  expectEqual(calls, ["Button@Button.ts", "Card@card.ts", "u:thing"], "module callees");

  const button = root.root().find({ rule: { kind: "identifier", regex: "^Button$" } });
  const def = button.definition();
  if (!def || basename(def.root.filename()) !== "Button.ts") {
    throw new Error(
      "definition() of the import should land in Button.ts, got " +
        (def ? def.root.filename() : "null"),
    );
  }
  return null;
}

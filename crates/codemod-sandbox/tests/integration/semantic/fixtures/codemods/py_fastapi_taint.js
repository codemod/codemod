// In repository.py: the term parameter flows into the SQL text.
function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}
export default function transform(root) {
  const sql = root.root().find({ rule: { pattern: "text($SQL)" } }).getMatch("SQL");
  expectEqual(
    sql.dependsOn((s) => s.kind === "parameter" && s.name === "term"),
    true,
    "sql depends on term",
  );
  const term = root.root().find({ rule: { kind: "identifier", regex: "^term$" } });
  expectEqual(
    term.reaches((s) => s.via === "argument" && s.name === "self.session.execute"),
    true,
    "term reaches session.execute",
  );
  // self.session: Session (annotated class attribute), so execute resolves.
  const execute = root.root().find({ rule: { pattern: "self.session.execute($$$)" } });
  expectEqual(
    execute.callees().map((c) => c.name + ":" + c.resolved),
    ["execute:true"],
    "self.session.execute resolves through the package re-export",
  );
  return null;
}

// In repository.py: the term parameter flows into the SQL text (a crawl of
// the sources lookup) and on into self.session.execute (a crawl of the uses
// lookup), and self.session.execute resolves in the callees lookup.
import { callGraph, crawl, dataflow } from "codemod:ast-grep";

function expectEqual(actual, expected, label) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      label + ": expected " + JSON.stringify(expected) + ", got " + JSON.stringify(actual),
    );
  }
}
export default async function transform(root) {
  const { sources, uses } = dataflow("python");
  const { callees } = callGraph("python");
  const sql = root.root().find({ rule: { pattern: "text($SQL)" } }).getMatch("SQL");
  expectEqual(
    [...(await crawl(sources, sql)).values()].some(
      (edge) => edge?.kind === "parameter" && edge.name === "term",
    ),
    true,
    "sql depends on term",
  );
  const term = root.root().find({ rule: { kind: "identifier", regex: "^term$" } });
  expectEqual(
    [...(await crawl(uses, term)).values()].some(
      (edge) => edge?.via === "argument" && edge.name === "self.session.execute",
    ),
    true,
    "term reaches session.execute",
  );
  // self.session: Session (annotated class attribute), so execute resolves
  // (an unresolved call would have no edge).
  const execute = root.root().find({ rule: { pattern: "self.session.execute($$$)" } });
  expectEqual(
    (callees.get(execute) ?? []).map((e) => e.node.field("name").text()),
    ["execute"],
    "self.session.execute resolves through the package re-export",
  );
  return null;
}

// f-string SQL built from request data -> a parameterized query:
// cursor.execute(f"... '{q}'") becomes cursor.execute("... %s", [q]).
// Only queries whose text a crawl of the sources lookup traces back to the
// `request` parameter are rewritten.
import { crawl, dataflow } from "codemod:ast-grep";

export default async function transform(root) {
  const { sources } = dataflow("python");
  const edits = [];
  for (const call of root.root().findAll({ rule: { pattern: "$C.execute($SQL)" } })) {
    const sql = call.getMatch("SQL");
    const reached = await crawl(sources, sql);
    const fromRequest = [...reached.values()].some(
      (edge) => edge?.kind === "parameter" && edge.name === "request",
    );
    if (!fromRequest) continue;
    if (sql.kind() !== "string") continue;
    let text = "";
    const params = [];
    for (const part of sql.children()) {
      if (part.kind() === "string_content") text += part.text();
      if (part.kind() === "interpolation") {
        text += "%s";
        params.push(part.children().find((c) => c.isNamed()).text());
      }
    }
    if (params.length === 0) continue;
    text = text.replaceAll("'%s'", "%s");
    edits.push(call.field("arguments").replace(`(${JSON.stringify(text)}, [${params.join(", ")}])`));
  }
  return edits.length ? root.root().commitEdits(edits) : null;
}

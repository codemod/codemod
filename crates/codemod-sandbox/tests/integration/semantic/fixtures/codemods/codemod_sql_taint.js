// SQL built by string concatenation from request data -> a parameterized
// query: `db.query("... id = " + id)` becomes `db.query("... id = ?", [id])`.
// Only queries whose text depends on the request (`req`) are rewritten.

// Split a "..." + x + "..." chain or a template literal into SQL text and parameters.
function parameterize(node, out = { sql: "", params: [] }) {
  if (node.kind() === "binary_expression" && node.field("operator")?.text() === "+") {
    parameterize(node.field("left"), out);
    parameterize(node.field("right"), out);
  } else if (node.kind() === "string") {
    out.sql += node.text().slice(1, -1);
  } else if (node.kind() === "template_string") {
    for (const part of node.children()) {
      if (part.kind() === "string_fragment") out.sql += part.text();
      if (part.kind() === "template_substitution") {
        out.sql += "?";
        out.params.push(
          part
            .children()
            .find((c) => c.isNamed())
            .text(),
        );
      }
    }
  } else if (node.kind() === "parenthesized_expression") {
    parameterize(
      node.children().find((c) => c.isNamed()),
      out,
    );
  } else {
    out.sql += "?";
    out.params.push(node.text());
  }
  return out;
}

export default function transform(root) {
  const edits = [];
  for (const call of root.root().findAll({ rule: { pattern: "$DB.query($SQL)" } })) {
    const sql = call.getMatch("SQL");
    const sources = sql.flowsFrom();
    const fromRequest = sources.some((s) => s.kind === "parameter" && s.name === "req");
    if (!fromRequest) continue;

    // The expression that builds the text: the argument, or the value the
    // variable passed as the argument was initialized with.
    let built = sql;
    if (sql.kind() === "identifier") {
      const init = sources.find((s) => s.via === "assign" && s.depth === 2);
      if (!init) continue;
      built = init.node;
    }
    const { sql: text, params } = parameterize(built);
    if (params.length === 0) continue;
    const literal = JSON.stringify(text);
    if (built === sql || built.text() === sql.text()) {
      edits.push(call.field("arguments").replace(`(${literal}, [${params.join(", ")}])`));
    } else {
      edits.push(built.replace(literal));
      edits.push(call.field("arguments").replace(`(${sql.text()}, [${params.join(", ")}])`));
    }
  }
  return edits.length ? root.root().commitEdits(edits) : null;
}

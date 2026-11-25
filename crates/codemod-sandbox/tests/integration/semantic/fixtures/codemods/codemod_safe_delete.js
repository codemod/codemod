// Delete exported functions nobody calls. A function is deleted only when
// the answer is complete: no callers, nothing cut off, every workspace file
// parsed, and no reference to it besides its own name (a reference means the
// function escapes as a value and can be called in ways the graph cannot see).
export default function transform(root) {
  const edits = [];
  for (const exp of root
    .root()
    .findAll({ rule: { kind: "export_statement", has: { kind: "function_declaration" } } })) {
    const fn = exp.find({ rule: { kind: "function_declaration" } });
    const name = fn.field("name");
    const callers = fn.callers();
    if (callers.truncated || (callers.unparsed && callers.unparsed.length > 0)) continue;
    if (callers.length > 0) continue;
    if (!callers.every((c) => c.resolved)) continue;

    const refs = name
      .references()
      .flatMap((file) =>
        file.nodes.filter(
          (n) =>
            !(
              file.root.filename() === root.filename() &&
              n.range().start.index === name.range().start.index
            ),
        ),
      );
    if (refs.length > 0) continue;

    // Remove the statement and the blank line after it.
    const next = exp.next();
    edits.push({
      startPos: exp.range().start.index,
      endPos: next ? next.range().start.index : exp.range().end.index,
      insertedText: "",
    });
  }
  return edits.length ? root.root().commitEdits(edits) : null;
}

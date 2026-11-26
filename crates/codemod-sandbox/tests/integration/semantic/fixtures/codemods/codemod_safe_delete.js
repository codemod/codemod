// Delete exported functions nobody calls: no edge in the callers lookup, and
// no edge in the references lookup besides its own name (a reference means
// the function escapes as a value and can be called in ways the graph cannot
// see).
import { callGraph, symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { callers } = callGraph("javascript");
  const { references } = symbols("javascript");
  const edits = [];
  for (const exp of root
    .root()
    .findAll({ rule: { kind: "export_statement", has: { kind: "function_declaration" } } })) {
    const fn = exp.find({ rule: { kind: "function_declaration" } });
    const name = fn.field("name");
    if (callers.has(fn)) continue;
    const refs = references.get(name) ?? [];
    if (refs.some((edge) => !edge.node.equals(name))) continue;

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

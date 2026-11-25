// moment -> an immutable date API. moment's `d.add(...)` changes `d` in
// place; an immutable API returns a new date. A call whose result is unused
// becomes `d = d.add(...)`. A change made through another variable, or inside
// a callback, cannot be rewritten in place: it gets a TODO comment instead.
const MUTATORS = ["add", "subtract", "startOf", "endOf", "set", "utc", "local"];

export default function transform(root) {
  const edits = [];
  for (const decl of root.root().findAll({ rule: { pattern: "const $NAME = $INIT" } })) {
    const name = decl.getMatch("NAME");
    if (name.kind() !== "identifier") continue;
    // Only variables created by moment(...) directly, not aliases of one.
    const created = (s) => s.kind === "call" && s.name === "moment";
    if (!name.dependsOn(created, { maxDepth: 1 })) continue;

    let reassigned = false;
    for (const m of name.mutations()) {
      if (m.kind !== "method-call" || !MUTATORS.includes(m.name)) continue;
      if (m.viaAlias) {
        edits.push(
          m.node.replace(
            `/* TODO(moment-immutable): mutates \`${name.text()}\` through an alias */ ${m.node.text()}`,
          ),
        );
      } else if (m.deferred) {
        edits.push(
          m.node.replace(
            `/* TODO(moment-immutable): deferred mutation of \`${name.text()}\` */ ${m.node.text()}`,
          ),
        );
      } else if (m.node.parent()?.kind() === "expression_statement") {
        edits.push(m.node.replace(`${name.text()} = ${m.node.text()}`));
        reassigned = true;
      }
    }
    if (reassigned) {
      edits.push(decl.replace("let" + decl.text().slice("const".length)));
    }
  }
  return edits.length ? root.root().commitEdits(edits) : null;
}

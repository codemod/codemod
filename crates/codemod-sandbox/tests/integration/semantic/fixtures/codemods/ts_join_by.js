// join() matches captures by what they resolve to where it can, and by text elsewhere.
import { join, symbols } from "codemod:ast-grep";

const count = (lookup) => [...lookup.values()].flat().length;
const registered = (by) =>
  join({
    language: "typescript",
    from: { find: "register($H)", on: "$H" },
    to: { find: "export function $H() { $$$ }", on: "$H" },
    by,
  });

export default function transform() {
  // Two files register a `handler`, each its own: text links each to both.
  const byText = count(registered("text"));
  const resolved = registered(undefined);
  const edges = [...resolved].map(([site, [edge]]) => site.text() + " -> " + edge.node.text().match(/"(\w)"/)[1]);
  const forced = count(registered(symbols("typescript").definition));
  // An event name is a string: nothing to resolve, so its text joins.
  const events = count(
    join({ language: "typescript", from: { find: "emit($E)", on: "$E" }, to: { find: "on($E, $$$)", on: "$E" } }),
  );
  const got = { byText, resolved: count(resolved), forced, events };
  const want = { byText: 4, resolved: 2, forced: 2, events: 1 };
  if (JSON.stringify(got) !== JSON.stringify(want)) throw new Error(JSON.stringify({ got, edges }));
}

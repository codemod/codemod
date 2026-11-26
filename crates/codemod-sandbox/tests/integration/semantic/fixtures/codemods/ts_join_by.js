// join() matches captures by what they resolve to where it can, and by text
// elsewhere. Two files each register their own `handler`.
import { join, symbols } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const count = (lookup) => [...lookup.values()].flat().length;
const registered = (by) =>
  join({
    language: "typescript",
    from: { find: "register($H)", on: "$H" },
    to: { find: "export function $H() { $$$ }", on: "$H" },
    by,
  });

test('join by "text" links each registration to every handler of that name', () => {
  equal(count(registered("text")), 4);
});

test("join without by resolves the captured names", () => {
  equal(count(registered(undefined)), 2);
});

test("join by a lookup resolves with it", () => {
  equal(count(registered(symbols("typescript").definition)), 2);
});

test("join falls back to text for a capture with nothing to resolve", () => {
  const events = join({
    language: "typescript",
    from: { find: "emit($E)", on: "$E" },
    to: { find: "on($E, $$$)", on: "$E" },
  });
  equal(count(events), 1);
});

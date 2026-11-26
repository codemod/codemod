// F1 (ergonomics review): uses through `import * as utils` must count, as
// edges in the references lookup and in the callers lookup, or a safe-delete
// codemod deletes functions that are still used.
import { callGraph, symbols } from "codemod:ast-grep";

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

// Who makes a call: the calling function's name, or "<module>" for a call at
// a module's top level, where the edge's node is the call itself.
function callerName(edge) {
  if (edge.node.equals(edge.site)) return "<module>";
  const own = edge.node.field("name");
  return own ? own.text() : "<anonymous>";
}

export default function transform(root) {
  const graph = callGraph("javascript");
  const { references } = symbols("javascript");
  const name = (fn) =>
    root
      .root()
      .find({
        rule: { kind: "function_declaration", has: { field: "name", regex: "^" + fn + "$" } },
      })
      .field("name");
  const refs = (fn) =>
    (references.get(name(fn)) ?? []).map(
      (e) => basename(e.node.getRoot().filename()) + ":" + e.node.text(),
    );
  const callers = (fn) =>
    (graph.callers.get(name(fn)) ?? []).map((e) => callerName(e) + ":" + e.via + " " + e.site.text());

  // Stored in an object: a reference, not a call.
  expectEqual(refs("oldRound"), ["app.js:oldRound"], "references of oldRound");
  expectEqual(callers("oldRound"), [], "callers of oldRound");

  // Handed to unknown code: a reference, and an assumed callback call.
  expectEqual(refs("registered"), ["app.js:registered"], "references of registered");
  expectEqual(
    callers("registered"),
    ["<module>:callback hooks.register(utils.registered)"],
    "callers of registered",
  );

  // utils?.["called"](): both.
  expectEqual(refs("called"), ["app.js:called"], "references of called");
  expectEqual(callers("called"), ['run:method utils?.["called"]()'], "callers of called");

  // Really unused: nothing either way.
  expectEqual(refs("unused"), [], "references of unused");
  expectEqual(callers("unused"), [], "callers of unused");
  return null;
}

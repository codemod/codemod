// F1 (ergonomics review): uses through `import * as utils` must count, or a
// safe-delete codemod deletes functions that are still used.
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

export default function transform(root) {
  const name = (fn) =>
    root
      .root()
      .find({
        rule: { kind: "function_declaration", has: { field: "name", regex: "^" + fn + "$" } },
      })
      .field("name");
  const refs = (fn) =>
    name(fn)
      .references()
      .flatMap((file) => file.nodes.map((n) => basename(file.root.filename()) + ":" + n.text()));
  const callers = (fn) =>
    name(fn)
      .callers()
      .map((c) => c.name + ":" + c.via + " " + c.callSite.node.text());

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

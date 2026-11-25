// references() and callers() see uses through a default import. The
// stale-flags demo deleted a live email template whose only uses were
// `import Welcome from "./emails/WelcomeEmail"`.
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
      .flatMap((file) => file.nodes.map((n) => basename(file.root.filename()) + ":" + n.text()))
      .sort();

  expectEqual(refs("WelcomeEmail"), ["send.tsx:Welcome", "send.tsx:Welcome"], "references");
  expectEqual(
    name("WelcomeEmail")
      .callers()
      .map((c) => c.name + "@" + basename(c.root.filename())),
    ["sendWelcome@send.tsx"],
    "callers",
  );
  // Really unused: nothing either way.
  expectEqual(refs("UnusedEmail"), [], "references of UnusedEmail");
  return null;
}

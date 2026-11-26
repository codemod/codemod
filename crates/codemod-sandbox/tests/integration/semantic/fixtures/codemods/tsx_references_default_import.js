// The references lookup and the callers lookup see uses through a default
// import.
// The stale-flags demo deleted a live email template whose only uses were
// `import Welcome from "./emails/WelcomeEmail"`.
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

export default function transform(root) {
  const { callers } = callGraph("tsx");
  const { references } = symbols("tsx");
  const name = (fn) =>
    root
      .root()
      .find({
        rule: { kind: "function_declaration", has: { field: "name", regex: "^" + fn + "$" } },
      })
      .field("name");
  const refs = (fn) =>
    (references.get(name(fn)) ?? [])
      .map((e) => basename(e.node.getRoot().filename()) + ":" + e.node.text())
      .sort();

  expectEqual(refs("WelcomeEmail"), ["send.tsx:Welcome", "send.tsx:Welcome"], "references");
  expectEqual(
    callers
      .get(name("WelcomeEmail"))
      .map((e) => e.node.field("name").text() + "@" + basename(e.node.getRoot().filename())),
    ["sendWelcome@send.tsx"],
    "callers",
  );
  // Really unused: nothing either way.
  expectEqual(refs("UnusedEmail"), [], "references of UnusedEmail");
  return null;
}

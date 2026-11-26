// The references and callers lookups see uses through a default import. The
// stale-flags demo deleted a live email template whose only uses were
// `import Welcome from "./emails/WelcomeEmail"`.
import { callGraph, symbols } from "codemod:ast-grep";
import { equal, test } from "./spec.js";
export { run as default } from "./spec.js";

const basename = (path) => path.split("/").pop();
const name = (root, fn) =>
  root
    .root()
    .find({ rule: { kind: "function_declaration", has: { field: "name", regex: "^" + fn + "$" } } })
    .field("name");
const refs = (root, fn) =>
  (symbols("tsx").references.get(name(root, fn)) ?? [])
    .map((e) => basename(e.node.getRoot().filename()) + ":" + e.node.text())
    .sort();

test("references of a default export include the default import and its uses", (root) => {
  equal(refs(root, "WelcomeEmail"), ["send.tsx:Welcome", "send.tsx:Welcome"]);
});

test("callers of a default export include calls through the default import", (root) => {
  equal(
    callGraph("tsx")
      .callers.get(name(root, "WelcomeEmail"))
      .map((e) => e.node.field("name").text() + "@" + basename(e.node.getRoot().filename())),
    ["sendWelcome@send.tsx"],
  );
});

test("an unused function has no references", (root) => {
  equal(refs(root, "UnusedEmail"), []);
});

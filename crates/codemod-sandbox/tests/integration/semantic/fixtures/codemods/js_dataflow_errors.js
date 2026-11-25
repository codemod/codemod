// Mistakes in a data-flow or call-graph query fail with a message that says
// why and what to do, instead of an empty answer.
import { parse } from "codemod:ast-grep";

function expectThrow(run, needle, label) {
  try {
    run();
  } catch (error) {
    if (!String(error.message).includes(needle)) {
      throw new Error(label + ": unexpected message: " + error.message);
    }
    return;
  }
  throw new Error(label + ": expected an error");
}

export default function transform(root) {
  const items = root.root().find({ rule: { pattern: "items" } });
  const audit = root.root().find({ rule: { pattern: "audit(total)" } });

  // Option typos and bad values.
  expectThrow(
    () => items.flowsFrom({ throughCall: false }),
    "unknown option `throughCall`",
    "typo",
  );
  expectThrow(
    () => items.callers({ transitve: true }),
    "expected `transitive` or `maxDepth`",
    "typo",
  );
  expectThrow(
    () => items.flowsTo({ maxDepth: 0 }),
    "maxDepth must be a whole number",
    "maxDepth 0",
  );
  expectThrow(
    () => items.flowsTo({ maxDepth: 1.5 }),
    "maxDepth must be a whole number",
    "maxDepth 1.5",
  );
  expectThrow(() => items.callees(true), "options must be an object", "not an object");

  // `after` must be a node of the same file.
  const step = items.flowsFrom()[0];
  expectThrow(() => items.mutations({ after: step }), "pass its `.node`", "after: a step");
  const other = parse("javascript", "audit(total)").root();
  expectThrow(
    () => items.mutations({ after: other }),
    "not in this node's file",
    "after: another file",
  );
  if (!Array.isArray(items.mutations({ after: audit }))) throw new Error("after: a node works");
  if (!Array.isArray(items.flowsFrom(undefined))) throw new Error("undefined options work");

  // A missing name is null, not undefined, as the types say.
  const escape = items.mutations().find((m) => m.kind === "escape");
  if (escape.name !== null) throw new Error("escape name: expected null, got " + escape.name);
  const literal = audit
    .field("arguments")
    .child(1)
    .flowsFrom()
    .find((s) => s.kind === "call");
  if (literal && literal.index !== null) throw new Error("call step index: expected null");

  // A root parsed from a string has no file; another language is not supported.
  const loose = parse("javascript", "const a = b;")
    .root()
    .find({ rule: { pattern: "b" } });
  expectThrow(() => loose.flowsFrom(), "parsed from a string", "string root");
  const python = parse("python", "a = b")
    .root()
    .find({ rule: { kind: "identifier" } });
  expectThrow(
    () => python.callers(),
    "JavaScript and TypeScript only, and this file is parsed as Python",
    "python",
  );

  return null;
}

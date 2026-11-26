// Mistakes in a data-flow or call-graph query, a lookup or a module query
// fail with a message that says why and what to do, instead of an empty
// answer.
import { callGraph, crawl, dataflow, parse, symbols } from "codemod:ast-grep";

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

export default async function transform(root) {
  const { callees, callers } = callGraph("javascript");
  const { sources, uses } = dataflow("javascript");
  const items = root.root().find({ rule: { pattern: "items" } });
  const audit = root.root().find({ rule: { pattern: "audit(total)" } });

  // A lookup takes a node, not options.
  expectThrow(() => callees.get(true), "callees: expected an SgNode", "not a node");

  // `after` must be a node of the same file.
  const step = sources.get(items)[0];
  expectThrow(() => items.mutations({ after: step }), "pass its `.node`", "after: a step");
  const other = parse("javascript", "audit(total)").root();
  expectThrow(
    () => items.mutations({ after: other }),
    "not in this node's file",
    "after: another file",
  );
  if (!Array.isArray(items.mutations({ after: audit }))) throw new Error("after: a node works");
  if (!(await crawl(sources, items, undefined)).has(items)) {
    throw new Error("undefined options work");
  }
  // A typo in crawl's options would crawl everything: it throws instead.
  let rejected = "";
  await crawl(sources, items, { stopat: [audit] }).catch((error) => (rejected = String(error.message)));
  if (!rejected.includes("unknown option `stopat`; expected `stopAt`")) {
    throw new Error("crawl options typo: " + (rejected || "no error"));
  }

  // A query on a node it does not apply to says so, instead of answering
  // about some expression inside it.
  const statement = root.root().find({ rule: { kind: "export_statement" } });
  expectThrow(
    () => sources.get(statement),
    "sources.get(node) on `export_statement`: there is no value here",
    "statement",
  );
  expectThrow(
    () => uses.get(statement),
    "on `export_statement`: there is no value here",
    "statement, uses lookup",
  );
  const gift = root.root().find({ rule: { pattern: '"gift"' } });
  expectThrow(() => gift.mutations(), "mutations() on `string`: this is not a variable", "literal");
  const member = root.root().find({ rule: { pattern: "cart.items" } });
  expectThrow(() => member.mutations(), "for `a.b`, ask about `a`", "member");
  // A part of a value answers for that value.
  const property = member.field("property");
  if (!sources.get(property)?.length) throw new Error("property of a member: expected an answer");

  // Module resolution needs workspace scope; this test runs in file scope.
  const { resolveModule, specifierFor } = symbols("javascript");
  expectThrow(
    () => resolveModule(root, "./db.js"),
    "resolveModule(): resolveModule: module resolution needs workspace scope",
    "resolveModule in file scope",
  );
  expectThrow(
    () => specifierFor(root, "./db.js"),
    "specifierFor(): specifierFor: module resolution needs workspace scope",
    "specifierFor in file scope",
  );

  // A missing name is null, not undefined, as the types say.
  const escape = items.mutations().find((m) => m.kind === "escape");
  if (escape.name !== null) throw new Error("escape name: expected null, got " + escape.name);
  const reached = await crawl(sources, audit.field("arguments").child(1));
  const literal = [...reached.values()].find((e) => e?.kind === "call");
  if (literal && literal.index !== null) throw new Error("call step index: expected null");

  // A root parsed from a string has no file, so no analysis covers it;
  // another language is not supported.
  const loose = parse("javascript", "const a = b;")
    .root()
    .find({ rule: { pattern: "b" } });
  expectThrow(() => sources.get(loose), "parsed from a string", "string root");
  expectThrow(() => uses.get(loose), "parsed from a string", "string root, uses lookup");
  expectThrow(
    () => resolveModule(loose, "./db.js"),
    "resolveModule(file): this root was parsed from a string and has no file",
    "string root, resolveModule",
  );
  const rust = parse("rust", "fn a() { b(); }")
    .root()
    .find({ rule: { kind: "identifier" } });
  expectThrow(
    () => callers.get(rust),
    "callers.get(node): this lookup is for JavaScript/TypeScript, and the node is Rust",
    "rust",
  );
  expectThrow(
    () => callGraph("rust"),
    'callGraph("rust"): semantic analysis covers "typescript", "tsx", "javascript" and "python"',
    "rust lookup",
  );

  return null;
}

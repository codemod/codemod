// An expression the analysis does not track (a type position) answers [];
// a statement or declaration still throws.
export default function transform(root) {
  const typeImport = root.root().find({ rule: { pattern: 'import("./a")' } });
  if (!typeImport) throw new Error("type-level import() not found");
  const steps = typeImport.flowsTo();
  if (!Array.isArray(steps) || steps.length !== 0) throw new Error("type position: expected []");
  if (typeImport.reaches(() => true)) throw new Error("type position: reaches should be false");

  const alias = root.root().find({ rule: { kind: "type_alias_declaration" } });
  let threw = false;
  try {
    alias.flowsFrom();
  } catch (error) {
    threw = String(error.message).includes("there is no value here");
  }
  if (!threw) throw new Error("declaration: expected a TypeError");

  // A tracked expression still answers.
  const call = root.root().find({ rule: { pattern: "f(1)" } });
  if (call.flowsTo().length === 0) throw new Error("f(1): expected steps");
  return null;
}

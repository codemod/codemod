// An expression the analysis does not track (a type position) has no uses
// edges and a crawl from it reaches only itself; a statement or declaration
// still throws.
import { crawl, dataflow } from "codemod:ast-grep";

export default async function transform(root) {
  const { sources, uses } = dataflow("typescript");
  const typeImport = root.root().find({ rule: { pattern: 'import("./a")' } });
  if (!typeImport) throw new Error("type-level import() not found");
  if (uses.get(typeImport) !== undefined) throw new Error("type position: expected no uses edges");
  const reached = await crawl(uses, typeImport);
  // So it reaches nothing: the crawl holds only its start.
  if (reached.size !== 1) throw new Error("type position: the crawl should reach only its start");

  const alias = root.root().find({ rule: { kind: "type_alias_declaration" } });
  let threw = false;
  try {
    sources.get(alias);
  } catch (error) {
    threw = String(error.message).includes(
      "sources.get(node) on `type_alias_declaration`: there is no value here",
    );
  }
  if (!threw) throw new Error("declaration: expected a TypeError");

  // A tracked expression still answers.
  const call = root.root().find({ rule: { pattern: "f(1)" } });
  if (!(uses.get(call)?.length > 0)) throw new Error("f(1): expected uses edges");
  return null;
}

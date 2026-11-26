// The references lookup on a declaration finds importers that reach it
// through barrels.
import { symbols } from "codemod:ast-grep";

// The edges' node texts, grouped by file.
function summarize(edges) {
  const out = {};
  for (const { node } of edges ?? []) {
    const file = node
      .getRoot()
      .filename()
      .replace(/\\/g, "/")
      .replace(/^.*\/packages\//, "packages/");
    (out[file] ??= []).push(node.text());
  }
  return out;
}

export default function transform(root) {
  const { references } = symbols("typescript");
  const decl = root.root().find({
    rule: { kind: "identifier", regex: "^toMoment$", inside: { kind: "function_declaration" } },
  });
  const got = summarize(references.get(decl));
  const want = {
    "packages/shared/src/index.ts": ["toMoment"],
    "packages/web/src/app.ts": ["toMoment", "toMoment"],
  };
  if (JSON.stringify(Object.keys(got).sort()) !== JSON.stringify(Object.keys(want).sort())) {
    throw new Error(`toMoment references: ${JSON.stringify(got)}`);
  }
  for (const [file, nodes] of Object.entries(want)) {
    if (JSON.stringify(got[file]) !== JSON.stringify(nodes)) {
      throw new Error(`toMoment references in ${file}: ${JSON.stringify(got)}`);
    }
  }

  const fmt = root.root().find({
    rule: { kind: "identifier", regex: "^formatDay$", inside: { kind: "function_declaration" } },
  });
  const fmtRefs = summarize(references.get(fmt));
  for (const file of [
    "packages/shared/src/index.ts",
    "packages/shared/src/local.ts",
    "packages/web/src/app.ts",
  ]) {
    if (!fmtRefs[file])
      throw new Error(`formatDay references missing ${file}: ${JSON.stringify(fmtRefs)}`);
  }
  return null;
}

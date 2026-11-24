// references() on a declaration finds importers that reach it through barrels.
function summarize(refs) {
  const out = {};
  for (const r of refs) {
    const file = r.root
      .filename()
      .replace(/\\/g, "/")
      .replace(/^.*\/packages\//, "packages/");
    out[file] = r.nodes.map((n) => n.text());
  }
  return out;
}

export default function transform(root) {
  const decl = root.root().find({
    rule: { kind: "identifier", regex: "^toMoment$", inside: { kind: "function_declaration" } },
  });
  const got = summarize(decl.references());
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
  const fmtRefs = summarize(fmt.references());
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

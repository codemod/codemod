// definition.get(node) on a name inside `export { … } from "…"` resolves to
// the original declaration instead of having no edge.
import { symbols } from "codemod:ast-grep";

const expected = {
  formatDay: { file: "packages/shared/src/dates.ts", name: "formatDay" },
  toMoment: { file: "packages/shared/src/dates.ts", name: "toMoment" },
  inner: { file: "packages/shared/src/inner.ts", name: "inner" },
  renamed: { file: "packages/shared/src/inner.ts", name: "inner" },
  viaImport: { file: "packages/shared/src/dates.ts", name: "formatDay" },
};

export default function transform(root) {
  const { definition, references } = symbols("typescript");
  const names = root
    .root()
    .findAll({ rule: { kind: "identifier", inside: { kind: "export_specifier" } } });
  if (names.length !== 5) throw new Error(`expected 5 specifier names, got ${names.length}`);
  for (const node of names) {
    const want = expected[node.text()];
    const def = definition.get(node)?.[0];
    if (!def) throw new Error(`${node.text()}: definition.get(node) returned no edge`);
    if (def.kind !== "external")
      throw new Error(`${node.text()}: expected external, got ${def.kind}`);
    const file = def.node.getRoot().filename().replace(/\\/g, "/");
    if (!file.endsWith(want.file))
      throw new Error(`${node.text()}: expected ${want.file}, got ${file}`);
    if (def.node.text() !== want.name) {
      throw new Error(
        `${node.text()}: expected ${want.name}, got ${def.node.kind()} ${JSON.stringify(def.node.text())}`,
      );
    }
  }

  // The references lookup from a re-export specifier matches the one from the declaration.
  const toMoment = names.find((n) => n.text() === "toMoment");
  const refs = references.get(toMoment) ?? [];
  const files = refs.map((e) => e.node.getRoot().filename().replace(/\\/g, "/")).sort();
  if (!files.some((f) => f.endsWith("packages/web/src/app.ts"))) {
    throw new Error(`references.get(node) from re-export missed app.ts: ${JSON.stringify(files)}`);
  }
  return null;
}

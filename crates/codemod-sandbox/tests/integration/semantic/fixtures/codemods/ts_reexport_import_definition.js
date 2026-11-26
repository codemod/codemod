// The definition lookup follows imported names through barrel re-exports
// (named, `export *`, renamed, and `import` + `export { x }`) to the original
// declaration, from the import specifier and from a call site alike.
import { symbols } from "codemod:ast-grep";

const expected = {
  toMoment: {
    file: "packages/shared/src/dates.ts",
    name: "toMoment",
    decl: "function_declaration",
  },
  deepFn: { file: "packages/shared/src/deep.ts", name: "deepFn", decl: "function_declaration" },
  renamed: { file: "packages/shared/src/inner.ts", name: "inner", decl: "variable_declarator" },
  viaImport: {
    file: "packages/shared/src/dates.ts",
    name: "formatDay",
    decl: "function_declaration",
  },
};

function check(label, def, want) {
  if (!def) throw new Error(`${label}: definition.get(node) returned no edge`);
  if (def.kind !== "external") throw new Error(`${label}: expected kind external, got ${def.kind}`);
  const file = def.node.getRoot().filename().replace(/\\/g, "/");
  if (!file.endsWith(want.file)) throw new Error(`${label}: expected ${want.file}, got ${file}`);
  if (def.node.text() !== want.name) {
    throw new Error(
      `${label}: expected node text ${want.name}, got ${def.node.kind()} ${JSON.stringify(def.node.text())}`,
    );
  }
  const parentKind = def.node.parent()?.kind();
  if (parentKind !== want.decl)
    throw new Error(`${label}: expected parent ${want.decl}, got ${parentKind}`);
}

export default function transform(root) {
  const { definition } = symbols("typescript");
  const specs = root.root().findAll({ rule: { kind: "import_specifier" } });
  if (specs.length !== 4) throw new Error(`expected 4 import specifiers, got ${specs.length}`);
  for (const spec of specs) {
    const name = spec.field("name");
    const want = expected[name.text()];
    const fromImport = definition.get(name)?.[0];
    check(`import ${name.text()}`, fromImport, want);

    // A call site lands on the declaration too, not on its import.
    const call = root.root().find({
      rule: {
        kind: "identifier",
        regex: `^${name.text()}$`,
        inside: { kind: "call_expression" },
      },
    });
    if (!call) throw new Error(`no call for ${name.text()}`);
    const fromCall = definition.get(call)?.[0];
    check(`call ${name.text()}`, fromCall, want);
    if (!fromCall.node.equals(fromImport.node))
      throw new Error(`call ${name.text()}: landed on another declaration than its import`);
  }
  return null;
}

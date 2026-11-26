// The definition and references lookups through barrel re-exports: named,
// `export *` (twice), renamed, and `import` + `export { x }`.
import { symbols } from "codemod:ast-grep";
import { equal, onFile, test } from "./spec.js";
export { run as default } from "./spec.js";

// A definition edge as kind, file, the node's text and its parent's kind.
const describe = (def) =>
  def && [
    def.kind,
    def.node
      .getRoot()
      .filename()
      .replace(/\\/g, "/")
      .replace(/^.*\/packages\//, ""),
    def.node.text(),
    def.node.parent()?.kind(),
  ];
const name = (root, text, inside) =>
  root.root().find({ rule: { kind: "identifier", regex: `^${text}$`, inside: { kind: inside } } });

onFile("packages/web/src/app.ts", () => {
  const forms = {
    "a named re-export": ["toMoment", "shared/src/dates.ts", "toMoment", "function_declaration"],
    "a chain of export *": ["deepFn", "shared/src/deep.ts", "deepFn", "function_declaration"],
    "a renamed re-export": ["renamed", "shared/src/inner.ts", "inner", "variable_declarator"],
    "an imported name exported again": [
      "viaImport",
      "shared/src/dates.ts",
      "formatDay",
      "function_declaration",
    ],
  };
  for (const [form, [imported, file, declared, parent]] of Object.entries(forms)) {
    test(`definition of an import through ${form} lands on the declaration`, (root) => {
      const [def] = symbols("typescript").definition.get(name(root, imported, "import_specifier"));
      equal(describe(def), ["external", file, declared, parent]);
    });
  }

  test("definition of a call lands where its import's does", (root) => {
    const { definition } = symbols("typescript");
    for (const imported of Object.values(forms).map(([n]) => n)) {
      const fromImport = definition.get(name(root, imported, "import_specifier"))[0];
      const fromCall = definition.get(name(root, imported, "call_expression"))[0];
      equal(fromCall.node.equals(fromImport.node), true, imported);
    }
  });
});

onFile("packages/shared/src/index.ts", () => {
  test("definition of a name in export { … } from lands on the declaration", (root) => {
    const { definition } = symbols("typescript");
    const names = root
      .root()
      .findAll({ rule: { kind: "identifier", inside: { kind: "export_specifier" } } });
    equal(
      names.map((n) => n.text() + " -> " + describe(definition.get(n)?.[0]).slice(0, 3).join(" ")),
      [
        "formatDay -> external shared/src/dates.ts formatDay",
        "toMoment -> external shared/src/dates.ts toMoment",
        "inner -> external shared/src/inner.ts inner",
        "renamed -> external shared/src/inner.ts inner",
        "viaImport -> external shared/src/dates.ts formatDay",
      ],
    );
  });

  test("references from a re-export specifier reach the importers", (root) => {
    const refs = symbols("typescript").references.get(name(root, "toMoment", "export_specifier"));
    equal(
      refs.some((e) => e.node.getRoot().filename().endsWith("packages/web/src/app.ts")),
      true,
    );
  });
});

onFile("packages/shared/src/dates.ts", () => {
  // The edges' node texts, by file, files sorted.
  const byFile = (edges) => {
    const out = {};
    for (const { node } of edges ?? []) {
      const file = node
        .getRoot()
        .filename()
        .replace(/\\/g, "/")
        .replace(/^.*\/packages\//, "");
      (out[file] ??= []).push(node.text());
    }
    return Object.fromEntries(Object.entries(out).sort());
  };

  test("references of a declaration reach importers through a barrel", (root) => {
    equal(
      byFile(symbols("typescript").references.get(name(root, "toMoment", "function_declaration"))),
      {
        "shared/src/index.ts": ["toMoment"],
        "web/src/app.ts": ["toMoment", "toMoment"],
      },
    );
  });

  test("references of a declaration reach a file that imports and re-exports it", (root) => {
    const files = Object.keys(
      byFile(symbols("typescript").references.get(name(root, "formatDay", "function_declaration"))),
    );
    for (const file of ["shared/src/index.ts", "shared/src/local.ts", "web/src/app.ts"]) {
      equal(files.includes(file), true, file);
    }
  });
});

import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import { getAllImports } from "../src/javascript/exports/imports.ts";
import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";

type Language = JS | TS | TSX;

function parseProgram<T extends Language>(lang: string, src: string) {
  const root = parse<T>(lang, src);
  return root.root();
}

// ============================================================================
// getAllImports tests
// ============================================================================
function testDefaultDynamicImport(lang: string) {
  const program = parseProgram(
    lang,
    "import('test').then(function(mod) {\n const pair = mod.test('test');\n });",
  );

  const res = getAllImports(program, { type: "default", from: "test" });
  assert(res.length === 1, "Should return exactly one result");
  assert(res[0]!.node.text() === "mod", "Node should reflect identifier");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm for dynamic()");
}

function testDefaultDynamicImportArrowFunction(lang: string) {
  const program = parseProgram(
    lang,
    "import('test').then(mod => { const pair = mod.test('test'); });",
  );

  let res = getAllImports(program, { type: "default", from: "test" });
  assert(res[0]!.node.text() === "mod", "Node should reflect identifier");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm for dynamic()");

  const programWithParenthesis = parseProgram(
    lang,
    "import('test').then((mod) => { const pair = mod.test('test'); });",
  );

  res = getAllImports(programWithParenthesis, { type: "default", from: "test" });
  assert(res[0]!.node.text() === "mod", "Node should reflect identifier");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm for dynamic()");
}

function testDefaultDynamicImportParameterBindingRestriction(lang: string) {
  const programWithBodyUsage = parseProgram(lang, "import('test').then((mod = fallback) => {})");

  const res = getAllImports(programWithBodyUsage, { type: "default", from: "test" });
  assert(res.length === 1, "Should only match the parameter binding once");
  assert(res[0]!.node.text() === "mod", "Node should reflect the parameter identifier");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm for dynamic()");
}

function testDirectCallbackRenamedDestructuring(lang: string) {
  for (const [property, options] of [
    ["value", { type: "named", name: "value", from: "test" }],
    ["default", { type: "default", from: "test" }],
  ] as const) {
    for (const callback of [
      `({ ${property}: alias }) => {}`,
      `function({ ${property}: alias }) {}`,
      `({ ${property}: alias } = {}) => {}`,
      `function({ ${property}: alias } = {}) {}`,
    ]) {
      const program = parseProgram(lang, `import('test').then(${callback})`);
      const res = getAllImports(program, options);
      assert(res.length === 1, `${lang}: should return one destructured callback binding`);
      assert(res[0]!.node.text() === "alias", `${lang}: should return the renamed binding`);
      assert(!res[0]!.isNamespace, `${lang}: destructured binding should not be a namespace`);
    }
  }
}

function testOnlyDirectCallbackBindings(lang: string) {
  const callbacks = [
    "function(mod) { function inner(value) {} }",
    "(mod) => { const inner = (value) => value; }",
    "(mod) => { const inner = ({ value }) => value; }",
    "mod => mod",
    "mod => { const { value: alias } = unrelated; }",
    "mod => { const { default: alias } = unrelated; }",
  ];

  for (const callback of callbacks) {
    const program = parseProgram(lang, `import('test').then(${callback})`);
    const res = getAllImports(program, { type: "default", from: "test" });
    assert(
      JSON.stringify(res.map((result) => result.node.text())) === JSON.stringify(["mod"]),
      `${lang}: should only return the import callback binding in ${callback}`,
    );
    assert(
      JSON.stringify(
        getAllImports(program, { type: "named", name: "value", from: "test" }).map((result) =>
          result.node.text(),
        ),
      ) === JSON.stringify(["mod"]),
      `${lang}: nested destructuring must not become an import`,
    );
  }
}

try {
  for (const lang of ["javascript", "typescript", "tsx"]) {
    testDefaultDynamicImport(lang);
    testDefaultDynamicImportArrowFunction(lang);
    testOnlyDirectCallbackBindings(lang);
    testDirectCallbackRenamedDestructuring(lang);
  }
  for (const lang of ["typescript", "tsx"]) {
    testDefaultDynamicImportParameterBindingRestriction(lang);
  }

  console.log("imports-ts.test.ts: all assertions passed");
} catch (error) {
  console.error(error);
  process.exit(1);
}

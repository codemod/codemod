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
function testDefaultDynamicImport() {
  const program = parseProgram(
    "typescript",
    "import('test').then(function(mod) {\n const pair = mod.test('test');\n });",
  );

  const res = getAllImports(program, { type: "default", from: "test" });
  assert(res.length === 1, "Should return exactly one result");
  assert(res[0]!.node.text() === "mod", "Node should reflect identifier");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm for dynamic()");
}

function testDefaultDynamicImportArrowFunction() {
  const program = parseProgram(
    "typescript",
    "import('test').then(mod => { const pair = mod.test('test'); });",
  );

  let res = getAllImports(program, { type: "default", from: "test" });
  assert(res[0]!.node.text() === "mod", "Node should reflect identifier");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm for dynamic()");

  const programWithParenthesis = parseProgram(
    "typescript",
    "import('test').then((mod) => { const pair = mod.test('test'); });",
  );

  res = getAllImports(programWithParenthesis, { type: "default", from: "test" });
  assert(res[0]!.node.text() === "mod", "Node should reflect identifier");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm for dynamic()");
}

function testDefaultDynamicImportParameterBindingRestriction() {
  // Test that only parameter bindings are matched, not other uses of the identifier
  const programWithBodyUsage = parseProgram(
    "typescript",
    "import('test').then((mod = fallback) => {})",
  );

  const res = getAllImports(programWithBodyUsage, { type: "default", from: "test" });
  assert(res.length === 1, "Should only match the parameter binding once");
  assert(res[0]!.node.text() === "mod", "Node should reflect the parameter identifier");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm for dynamic()");
}

try {
  testDefaultDynamicImport();
  testDefaultDynamicImportArrowFunction();
  testDefaultDynamicImportParameterBindingRestriction();

  console.log("imports-ts.test.ts: all assertions passed");
} catch (error) {
  console.error(error);
  process.exit(1);
}

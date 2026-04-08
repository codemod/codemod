import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import { getImport, addImport, removeImport } from "../src/javascript/exports/imports.ts";
import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";

type Language = JS | TS | TSX;

function parseProgram<T extends Language>(lang: string, src: string) {
  const root = parse<T>(lang, src);
  return root.root();
}

function testReturnsNullWhenNoMatches() {
  const program = parseProgram("javascript", "const x = 1;\nconsole.log(x);\n");
  const resDefault = getImport(program, {
    type: "default",
    from: "mod",
  });
  assert(resDefault === null, "Expected null for default when no matches");

  const resNamed = getImport(program, {
    type: "named",
    name: "x",
    from: "mod",
  });
  assert(resNamed === null, "Expected null for named when no matches");
}

function testDefaultImportFromDEFAULT_NAME() {
  const program = parseProgram("javascript", "import foo from 'mod';\nconsole.log(foo);\n");
  const res = getImport(program, { type: "default", from: "mod" });
  assert(res !== null, "Expected a result for default import");
  assert(res!.alias === "foo", "Alias should be DEFAULT_NAME value");
  assert(res!.isNamespace === false, "isNamespace should be false");
  assert(res!.moduleType === "esm", "moduleType should be esm for import statement");
  assert(
    typeof res!.node.text === "function" && res!.node.text() === "foo",
    "Node should reflect identifier",
  );
}

function testDefaultImportFromVAR_NAME() {
  const program = parseProgram("javascript", "const bar = require('mod');\nconsole.log(bar);\n");
  const res = getImport(program, { type: "default", from: "mod" });
  assert(res !== null, "Expected a result for var-based import");
  assert(res!.alias === "bar", "Alias should be VAR_NAME value");
  assert(res!.moduleType === "cjs", "moduleType should be cjs for require()");
}

function testNamedImportWithAlias() {
  const program = parseProgram(
    "javascript",
    "import { baz as qux } from 'mod';\nconsole.log(qux);\n",
  );
  const res = getImport(program, {
    type: "named",
    name: "baz",
    from: "mod",
  });
  assert(res !== null, "Expected a result for named import with alias");
  assert(res!.alias === "qux", "Alias should use ALIAS when present");
  assert(res!.moduleType === "esm", "moduleType should be esm for named import");
  assert(res!.node.text() === "qux", "Node should be alias identifier when aliased");
}

function testNamedImportWithoutAlias() {
  const program = parseProgram("javascript", "import { q } from 'mod';\nconsole.log(q);\n");
  const res = getImport(program, {
    type: "named",
    name: "q",
    from: "mod",
  });
  assert(res !== null, "Expected a result for named import without alias");
  assert(res!.alias === "q", "Alias should fallback to ORIGINAL when no alias");
  assert(res!.moduleType === "esm", "moduleType should be esm for named import");
  assert(res!.node.text() === "q", "Node should be original identifier when no alias");
}

function testNamedImportNotFound() {
  const program = parseProgram("javascript", "import { alpha } from 'mod';\nconsole.log(alpha);\n");
  const res = getImport(program, {
    type: "named",
    name: "beta",
    from: "mod",
  });
  assert(res === null, "Expected null when requested named import does not exist");
}

function testDynamicImportModuleType() {
  const program = parseProgram(
    "javascript",
    "const foo = await import('mod');\nconsole.log(foo);\n",
  );
  const res = getImport(program, { type: "default", from: "mod" });
  assert(res !== null, "Expected a result for dynamic import");
  assert(res!.alias === "foo", "Alias should be variable name");
  assert(res!.moduleType === "esm", "moduleType should be esm for dynamic import()");
}

function testDestructuredRequireModuleType() {
  const program = parseProgram(
    "javascript",
    "const { bar } = require('mod');\nconsole.log(bar);\n",
  );
  const res = getImport(program, { type: "named", name: "bar", from: "mod" });
  assert(res !== null, "Expected a result for destructured require");
  assert(res!.alias === "bar", "Alias should be destructured name");
  assert(res!.moduleType === "cjs", "moduleType should be cjs for destructured require()");
}

function testDestructuredDynamicImportModuleType() {
  const program = parseProgram(
    "javascript",
    "const { baz } = await import('mod');\nconsole.log(baz);\n",
  );
  const res = getImport(program, { type: "named", name: "baz", from: "mod" });
  assert(res !== null, "Expected a result for destructured dynamic import");
  assert(res!.alias === "baz", "Alias should be destructured name");
  assert(res!.moduleType === "esm", "moduleType should be esm for destructured dynamic import()");
}

function testNamespaceImportModuleType() {
  const program = parseProgram("javascript", "import * as ns from 'mod';\nconsole.log(ns);\n");
  const res = getImport(program, { type: "default", from: "mod" });
  assert(res !== null, "Expected a result for namespace import");
  assert(res!.alias === "ns", "Alias should be namespace name");
  assert(res!.isNamespace === true, "isNamespace should be true");
  assert(res!.moduleType === "esm", "moduleType should be esm for namespace import");
}

function testSingleNamespaceImport_StillWorks() {
  // Baseline: getImport with a single namespace import is unaffected
  const program = parseProgram("javascript", "import * as ns from 'mod';\nconsole.log(ns);\n");

  const res = getImport(program, { type: "default", from: "mod" });
  assert(res !== null, "Should return a result");
  assert(res!.isNamespace === true, "Should be a namespace import");
  assert(res!.alias === "ns", "Alias should be ns");
  assert(res!.moduleType === "esm", "moduleType should be esm");
}

function testMultipleNamespaceImports_ReturnsFirstOnly() {
  // Key behavioural difference: getImport preserves its single-result contract
  // and returns only the first namespace import even when multiple exist
  const program = parseProgram(
    "javascript",
    ["import * as nsA from 'mod';", "import * as nsB from 'mod';", "console.log(nsA, nsB);"].join(
      "\n",
    ),
  );

  const res = getImport(program, { type: "default", from: "mod" });
  assert(res !== null, "Should return a result");
  assert(res!.isNamespace === true, "Should be a namespace import");
  assert(res!.alias === "nsA", "Should return only the first namespace import in source order");
}

function testMultipleNamespaceImports_NamedQuery_ReturnsFirstOnly() {
  // Named query also falls back to namespace — getImport returns only the first
  const program = parseProgram(
    "javascript",
    ["import * as nsA from 'mod';", "import * as nsB from 'mod';", "console.log(nsA, nsB);"].join(
      "\n",
    ),
  );

  const res = getImport(program, { type: "named", name: "anything", from: "mod" });
  assert(res !== null, "Should return a result");
  assert(res!.isNamespace === true, "Should be a namespace import");
  assert(res!.alias === "nsA", "Should return only the first namespace import");
}

function testNamespaceNotReturnedAlongsideTypedResults() {
  // When a real typed match exists, namespace fallback must not appear in getImport
  const program = parseProgram(
    "javascript",
    [
      "import foo from 'mod';",
      "import * as nsA from 'mod';",
      "import * as nsB from 'mod';",
      "console.log(foo, nsA, nsB);",
    ].join("\n"),
  );

  const res = getImport(program, { type: "default", from: "mod" });
  assert(res !== null, "Should return a result");
  assert(res!.alias === "foo", "Should be the default import, not a namespace");
  assert(res!.isNamespace === false, "Should not be a namespace result");
}

// ============================================================================
// addImport tests
// ============================================================================

function testAddDefaultImportESM() {
  const program = parseProgram("javascript", "console.log('hello');\n");
  const edit = addImport(program, {
    type: "default",
    name: "foo",
    from: "mod",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(result.includes("import foo from 'mod'"), "Should add ESM default import");
}

function testAddDefaultImportCJS() {
  const program = parseProgram("javascript", "console.log('hello');\n");
  const edit = addImport(program, {
    type: "default",
    name: "bar",
    from: "mod",
    moduleType: "cjs",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(result.includes("const bar = require('mod')"), "Should add CJS require");
}

function testAddNamespaceImport() {
  const program = parseProgram("javascript", "console.log('hello');\n");
  const edit = addImport(program, {
    type: "namespace",
    name: "ns",
    from: "mod",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(result.includes("import * as ns from 'mod'"), "Should add namespace import");
}

function testAddNamedImportESM() {
  const program = parseProgram("javascript", "console.log('hello');\n");
  const edit = addImport(program, {
    type: "named",
    specifiers: [{ name: "foo" }, { name: "bar", alias: "baz" }],
    from: "mod",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result.includes("import { foo, bar as baz } from 'mod'"),
    "Should add ESM named import with alias",
  );
}

function testAddNamedImportCJS() {
  const program = parseProgram("javascript", "console.log('hello');\n");
  const edit = addImport(program, {
    type: "named",
    specifiers: [{ name: "x" }, { name: "y", alias: "z" }],
    from: "mod",
    moduleType: "cjs",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result.includes("const { x, y: z } = require('mod')"),
    "Should add CJS destructured require",
  );
}

function testAddImportSkipsExistingDefault() {
  const program = parseProgram("javascript", "import foo from 'mod';\nconsole.log(foo);\n");
  const edit = addImport(program, {
    type: "default",
    name: "bar",
    from: "mod",
  });
  assert(edit === null, "Should return null when default import exists");
}

function testAddImportSkipsExistingNamed() {
  const program = parseProgram("javascript", "import { foo } from 'mod';\nconsole.log(foo);\n");
  const edit = addImport(program, {
    type: "named",
    specifiers: [{ name: "foo" }],
    from: "mod",
  });
  assert(edit === null, "Should return null when named import exists");
}

function testAddImportMergesNamedSpecifiers() {
  const program = parseProgram("javascript", "import { foo } from 'mod';\nconsole.log(foo);\n");
  const edit = addImport(program, {
    type: "named",
    specifiers: [{ name: "bar" }],
    from: "mod",
  });
  assert(edit !== null, "Should return an edit to merge");
  const result = program.commitEdits([edit!]);
  // Check that both foo and bar are in the same import statement
  assert(
    result.includes("foo") && result.includes("bar") && result.includes("from 'mod'"),
    "Should merge bar into existing named imports",
  );
  // Make sure we didn't create a new import statement
  assert((result.match(/import/g) || []).length === 1, "Should have only one import statement");
}

function testAddImportAfterExisting() {
  const program = parseProgram("javascript", "import x from 'other';\nconsole.log(x);\n");
  const edit = addImport(program, {
    type: "default",
    name: "foo",
    from: "mod",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  // The new import should come after the existing one
  const otherIdx = result.indexOf("import x from 'other'");
  const modIdx = result.indexOf("import foo from 'mod'");
  assert(modIdx > otherIdx, "New import should be after existing import");
}

// ============================================================================
// removeImport tests
// ============================================================================

function testRemoveDefaultImportESM() {
  const program = parseProgram("javascript", "import foo from 'mod';\nconsole.log(foo);\n");
  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("import foo from 'mod'"), "Should remove the import statement");
  assert(result.includes("console.log"), "Should keep other code");
}

function testRemoveNamespaceImport() {
  const program = parseProgram("javascript", "import * as ns from 'mod';\nconsole.log(ns);\n");
  const edit = removeImport(program, { type: "namespace", from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("import * as ns from 'mod'"), "Should remove namespace import");
}

function testRemoveNamedImportSpecific() {
  const program = parseProgram(
    "javascript",
    "import { foo, bar } from 'mod';\nconsole.log(foo, bar);\n",
  );
  const edit = removeImport(program, {
    type: "named",
    specifiers: ["foo"],
    from: "mod",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  // Check that foo is not in the import statement (it's still used in console.log)
  assert(!result.includes("import { foo"), "Should remove foo from import");
  assert(
    result.includes("bar") && result.includes("from 'mod'"),
    "Should keep bar specifier in import",
  );
}

function testRemoveNamedImportLast() {
  const program = parseProgram("javascript", "import { foo } from 'mod';\nconsole.log(foo);\n");
  const edit = removeImport(program, {
    type: "named",
    specifiers: ["foo"],
    from: "mod",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("import"), "Should remove entire import when last specifier removed");
}

function testRemoveImportNotFound() {
  const program = parseProgram("javascript", "console.log('hello');\n");
  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(edit === null, "Should return null when import not found");
}

function testRemoveDefaultCJS() {
  const program = parseProgram("javascript", "const foo = require('mod');\nconsole.log(foo);\n");
  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(edit !== null, "Should return an edit for CJS");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("require"), "Should remove require statement");
}

function testRemoveDefault_SingleDeclarator_StillWorks() {
  // Baseline: normal single-declarator CJS removal is unaffected by the guard
  const program = parseProgram("javascript", "const foo = require('mod');\nconsole.log(foo);\n");

  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(edit !== null, "Should return an edit for a normal single-declarator require");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("require"), "Should remove the require statement");
  assert(result.includes("console.log"), "Should keep unrelated code");
}

function testRemoveDefault_MultiDeclarator_ReturnsNull() {
  // Core safety behaviour: `const foo = require('mod'), x = 1` must NOT be
  // removed - removeImport should return null rather than delete `x`
  const program = parseProgram(
    "javascript",
    "const foo = require('mod'), x = 1;\nconsole.log(foo, x);\n",
  );

  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(
    edit === null,
    "Should return null for multi-declarator CJS — removing the whole statement would delete unrelated bindings",
  );
}

function testRemoveDefault_MultiDeclarator_SourceCodeUnchanged() {
  // Companion to the above: verify that when null is returned, no code is modified
  const src = "const foo = require('mod'), x = 1;\nconsole.log(foo, x);\n";
  const program = parseProgram("javascript", src);

  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(edit === null, "edit should be null");
  // No commitEdits call - source is untouched by definition when edit is null
  assert(program.text() === src, "Program source should be unchanged");
}

function testRemoveDefault_MultiDeclarator_UnrelatedModuleUnaffected() {
  // A multi-declarator declaration for one module must not interfere with
  // normal single-declarator removal of a different module in the same file
  const program = parseProgram(
    "javascript",
    [
      "const foo = require('mod'), x = 1;",
      "const bar = require('other');",
      "console.log(foo, x, bar);",
    ].join("\n"),
  );

  const editMod = removeImport(program, { type: "default", from: "mod" });
  assert(editMod === null, "Multi-declarator mod should still be null");

  const editOther = removeImport(program, { type: "default", from: "other" });
  assert(editOther !== null, "Single-declarator other should produce an edit");
  const result = program.commitEdits([editOther!]);
  assert(!result.includes("require('other')"), "Should remove the single-declarator require");
  assert(result.includes("require('mod')"), "Should leave the multi-declarator require intact");
}

function run() {
  // getImport tests
  testReturnsNullWhenNoMatches();
  testDefaultImportFromDEFAULT_NAME();
  testDefaultImportFromVAR_NAME();
  testNamedImportWithAlias();
  testNamedImportWithoutAlias();
  testNamedImportNotFound();
  testDynamicImportModuleType();
  testDestructuredRequireModuleType();
  testDestructuredDynamicImportModuleType();
  testNamespaceImportModuleType();
  testSingleNamespaceImport_StillWorks();
  testMultipleNamespaceImports_ReturnsFirstOnly();
  testMultipleNamespaceImports_NamedQuery_ReturnsFirstOnly();
  testNamespaceNotReturnedAlongsideTypedResults();

  // addImport tests
  testAddDefaultImportESM();
  testAddDefaultImportCJS();
  testAddNamespaceImport();
  testAddNamedImportESM();
  testAddNamedImportCJS();
  testAddImportSkipsExistingDefault();
  testAddImportSkipsExistingNamed();
  testAddImportMergesNamedSpecifiers();
  testAddImportAfterExisting();

  // removeImport tests
  testRemoveDefaultImportESM();
  testRemoveNamespaceImport();
  testRemoveNamedImportSpecific();
  testRemoveNamedImportLast();
  testRemoveImportNotFound();
  testRemoveDefaultCJS();
  testRemoveDefault_SingleDeclarator_StillWorks();
  testRemoveDefault_MultiDeclarator_ReturnsNull();
  testRemoveDefault_MultiDeclarator_SourceCodeUnchanged();
  testRemoveDefault_MultiDeclarator_UnrelatedModuleUnaffected();

  console.log("imports.test.ts: all assertions passed");
}

try {
  run();
} catch (error) {
  console.error(error);
  process.exit(1);
}

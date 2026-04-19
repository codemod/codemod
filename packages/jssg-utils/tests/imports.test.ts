import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import {
  getImport,
  addImport,
  removeImport,
  getAllImports,
} from "../src/javascript/exports/imports.ts";
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
function testReturnsEmptyArrayWhenNoImports() {
  const program = parseProgram("javascript", "const x = 1;\nconsole.log(x);\n");

  const resDefault = getAllImports(program, { type: "default", from: "mod" });
  assert(Array.isArray(resDefault), "Should return an array");
  assert(resDefault.length === 0, "Should be empty when no imports exist");

  const resNamed = getAllImports(program, { type: "named", name: "x", from: "mod" });
  assert(Array.isArray(resNamed), "Should return an array");
  assert(resNamed.length === 0, "Should be empty when no imports exist");
}

function testReturnsEmptyArrayWhenModuleNotImported() {
  const program = parseProgram("javascript", "import foo from 'other';\nconsole.log(foo);\n");

  const res = getAllImports(program, { type: "default", from: "mod" });
  assert(res.length === 0, "Should be empty when the requested module is not imported");
}

function testReturnsEmptyArrayWhenNamedSpecifierNotFound() {
  const program = parseProgram("javascript", "import { alpha } from 'mod';\nconsole.log(alpha);\n");

  const res = getAllImports(program, { type: "named", name: "beta", from: "mod" });
  assert(res.length === 0, "Should be empty when the requested named specifier does not exist");
}

function testSingleDefaultESMImport() {
  const program = parseProgram("javascript", "import foo from 'mod';\nconsole.log(foo);\n");

  const res = getAllImports(program, { type: "default", from: "mod" });
  assert(res.length === 1, "Should return exactly one result");
  assert(res[0]!.alias === "foo", "Alias should be the default import name");
  assert(res[0]!.isNamespace === false, "isNamespace should be false");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm");
  assert(res[0]!.node.text() === "foo", "Node should reflect identifier");
}

function testSingleDefaultCJSImport() {
  const program = parseProgram("javascript", "const bar = require('mod');\nconsole.log(bar);\n");

  const res = getAllImports(program, { type: "default", from: "mod" });
  assert(res.length === 1, "Should return exactly one result");
  assert(res[0]!.alias === "bar", "Alias should be the variable name");
  assert(res[0]!.moduleType === "cjs", "moduleType should be cjs for require()");
}

function testSingleNamedImportWithAlias() {
  const program = parseProgram(
    "javascript",
    "import { baz as qux } from 'mod';\nconsole.log(qux);\n",
  );

  const res = getAllImports(program, { type: "named", name: "baz", from: "mod" });
  assert(res.length === 1, "Should return exactly one result");
  assert(res[0]!.alias === "qux", "Alias should use the local alias");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm");
  assert(res[0]!.node.text() === "qux", "Node should be the alias identifier");
}

function testSingleNamedImportWithoutAlias() {
  const program = parseProgram("javascript", "import { q } from 'mod';\nconsole.log(q);\n");

  const res = getAllImports(program, { type: "named", name: "q", from: "mod" });
  assert(res.length === 1, "Should return exactly one result");
  assert(res[0]!.alias === "q", "Alias should fall back to the original name");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm");
}

function testSingleDestructuredCJSImport() {
  const program = parseProgram(
    "javascript",
    "const { bar } = require('mod');\nconsole.log(bar);\n",
  );

  const res = getAllImports(program, { type: "named", name: "bar", from: "mod" });
  assert(res.length === 1, "Should return exactly one result");
  assert(res[0]!.alias === "bar", "Alias should be the destructured name");
  assert(res[0]!.moduleType === "cjs", "moduleType should be cjs");
}

function testMultipleDefaultImports_ESMandCJS() {
  const program = parseProgram(
    "javascript",
    ["import foo from 'mod';", "const bar = require('mod');", "console.log(foo, bar);"].join("\n"),
  );

  const res = getAllImports(program, { type: "default", from: "mod" });
  assert(res.length === 2, "Should return both the ESM and CJS default imports");

  const esmResult = res.find((r) => r.moduleType === "esm");
  const cjsResult = res.find((r) => r.moduleType === "cjs");

  assert(esmResult !== undefined, "Should include the ESM import");
  assert(esmResult!.alias === "foo", "ESM alias should be foo");

  assert(cjsResult !== undefined, "Should include the CJS import");
  assert(cjsResult!.alias === "bar", "CJS alias should be bar");
}

function testMultipleNamedImports_ESMandCJS() {
  const program = parseProgram(
    "javascript",
    [
      "import { helper } from 'mod';",
      "const { helper: helperCJS } = require('mod');",
      "console.log(helper, helperCJS);",
    ].join("\n"),
  );

  const res = getAllImports(program, { type: "named", name: "helper", from: "mod" });
  assert(res.length === 2, "Should return both ESM and CJS named imports");

  const esmResult = res.find((r) => r.moduleType === "esm");
  const cjsResult = res.find((r) => r.moduleType === "cjs");

  assert(esmResult !== undefined, "Should include the ESM named import");
  assert(esmResult!.alias === "helper", "ESM alias should be helper (no alias)");

  assert(cjsResult !== undefined, "Should include the CJS named import");
  assert(cjsResult!.alias === "helperCJS", "CJS alias should be the renamed binding");
}

function testMultipleNamedImports_SameModuleDifferentAliases() {
  const program = parseProgram(
    "javascript",
    [
      "import { util as utilA } from 'mod';",
      "import { util as utilB } from 'mod';",
      "console.log(utilA, utilB);",
    ].join("\n"),
  );

  const res = getAllImports(program, { type: "named", name: "util", from: "mod" });
  assert(res.length === 2, "Should return both aliased imports of the same specifier");

  const aliases = res.map((r) => r.alias).sort();
  assert(aliases[0] === "utilA" && aliases[1] === "utilB", "Should capture both local aliases");

  assert(
    res.every((r) => r.moduleType === "esm"),
    "Both should be esm",
  );
}

function testMultipleDefaultImports_OnlyReturnsMatchingModule() {
  const program = parseProgram(
    "javascript",
    [
      "import foo from 'mod';",
      "const bar = require('mod');",
      "import unrelated from 'other';",
      "console.log(foo, bar, unrelated);",
    ].join("\n"),
  );

  const res = getAllImports(program, { type: "default", from: "mod" });
  assert(res.length === 2, "Should return only imports from the requested module");
  assert(
    res.every((r) => r.alias !== "unrelated"),
    "Should not include imports from other modules",
  );
}

function testNamespaceFallback_WhenNoTypedMatchFound_DefaultQuery() {
  const program = parseProgram("javascript", "import * as ns from 'mod';\nconsole.log(ns);\n");

  const res = getAllImports(program, { type: "default", from: "mod" });
  assert(res.length === 1, "Should return the namespace import as fallback");
  assert(res[0]!.isNamespace === true, "isNamespace should be true");
  assert(res[0]!.alias === "ns", "Alias should be the namespace binding name");
  assert(res[0]!.moduleType === "esm", "Namespace imports are always ESM");
}

function testNamespaceFallback_WhenNoTypedMatchFound_NamedQuery() {
  const program = parseProgram("javascript", "import * as ns from 'mod';\nconsole.log(ns);\n");

  const res = getAllImports(program, { type: "named", name: "something", from: "mod" });
  assert(res.length === 1, "Should return the namespace import as fallback");
  assert(res[0]!.isNamespace === true, "isNamespace should be true");
  assert(res[0]!.alias === "ns", "Alias should be the namespace binding name");
}

function testNamespaceNotReturnedWhenTypedResultsExist() {
  const program = parseProgram(
    "javascript",
    ["import foo from 'mod';", "import * as ns from 'mod';", "console.log(foo, ns);"].join("\n"),
  );

  const res = getAllImports(program, { type: "default", from: "mod" });
  assert(res.length === 1, "Should return only the default import, not the namespace too");
  assert(res[0]!.alias === "foo", "Should return the default import");
  assert(res[0]!.isNamespace === false, "Should not be the namespace result");
}

function testSingleNamespaceImport_getAllImports_StillWorks() {
  // Baseline: single namespace import still comes back as a one-element array
  const program = parseProgram("javascript", "import * as ns from 'mod';\nconsole.log(ns);\n");

  const res = getAllImports(program, { type: "default", from: "mod" });
  assert(res.length === 1, "Should return exactly one result");
  assert(res[0]!.isNamespace === true, "Should be a namespace import");
  assert(res[0]!.alias === "ns", "Alias should be ns");
  assert(res[0]!.moduleType === "esm", "moduleType should be esm");
}

function testMultipleNamespaceImports_getAllImports_AllReturned() {
  // Core new behaviour: getAllImports returns both
  const program = parseProgram(
    "javascript",
    ["import * as nsA from 'mod';", "import * as nsB from 'mod';", "console.log(nsA, nsB);"].join(
      "\n",
    ),
  );

  const res = getAllImports(program, { type: "default", from: "mod" });
  assert(res.length === 2, "Should return both namespace imports");
  assert(
    res.every((r) => r.isNamespace === true),
    "Both should have isNamespace true",
  );
  assert(
    res.every((r) => r.moduleType === "esm"),
    "Both should be esm",
  );

  const aliases = res.map((r) => r.alias).sort();
  assert(aliases[0] === "nsA" && aliases[1] === "nsB", "Should capture both aliases");
}

function testMultipleNamespaceImports_NamedQuery_getAllImports_AllReturned() {
  // Named query also falls back to namespace — getAllImports returns all
  const program = parseProgram(
    "javascript",
    ["import * as nsA from 'mod';", "import * as nsB from 'mod';", "console.log(nsA, nsB);"].join(
      "\n",
    ),
  );

  const res = getAllImports(program, { type: "named", name: "anything", from: "mod" });
  assert(res.length === 2, "Named query should also get all namespace imports as fallback");
  assert(
    res.every((r) => r.isNamespace === true),
    "Both should be namespace",
  );
}

function testNamespaceNotReturnedAlongsideTypedResults_getAllImports() {
  // getAllImports also suppresses namespace when typed results exist
  const program = parseProgram(
    "javascript",
    [
      "import foo from 'mod';",
      "import * as nsA from 'mod';",
      "import * as nsB from 'mod';",
      "console.log(foo, nsA, nsB);",
    ].join("\n"),
  );

  const res = getAllImports(program, { type: "default", from: "mod" });
  assert(res.length === 1, "Should return only the default import, not the namespace imports");
  assert(res[0]!.alias === "foo", "Should be the default import");
  assert(res[0]!.isNamespace === false, "Should not be a namespace result");
}

// ============================================================================
// getImport tests
// ============================================================================

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

function testAddImportAfterExistingKeepsSeparateLines() {
  const source = [
    "import a from 'a';",
    "import b from 'b';",
    "import c from 'c';",
    "console.log(a, b, c);",
    "",
  ].join("\n");
  const program = parseProgram("javascript", source);
  const edit = addImport(program, {
    type: "default",
    name: "foo",
    from: "mod",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result ===
      [
        "import a from 'a';",
        "import b from 'b';",
        "import c from 'c';",
        "import foo from 'mod';",
        "console.log(a, b, c);",
        "",
      ].join("\n"),
    "New import should be inserted on its own line after existing imports",
  );
}

function testAddImportAfterMixedImportsUsesLastSourcePosition() {
  const source = ["const a = require('a');", "import b from 'b';", "console.log(a, b);", ""].join(
    "\n",
  );
  const program = parseProgram("javascript", source);
  const edit = addImport(program, {
    type: "default",
    name: "foo",
    from: "mod",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result ===
      [
        "const a = require('a');",
        "import b from 'b';",
        "import foo from 'mod';",
        "console.log(a, b);",
        "",
      ].join("\n"),
    "New import should be inserted after the last import by source position",
  );
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

/** Before removeImport supported `variable_declaration`, this returned null (only lexical_declaration was matched). */
function testRemoveDefaultVarCJS() {
  const program = parseProgram("javascript", "var foo = require('mod');\nconsole.log(foo);\n");
  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(edit !== null, "Should return an edit for var + require");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("require"), "Should remove var require statement");
  assert(result.includes("console.log(foo)"), "Should keep usage");
}

/** Bare `require('mod')` has no binding, so getImport is null; removal required `removeSideEffectForms`. */
function testRemoveBareRequireOnlyWithSideEffectFlag() {
  const src = "require('mod');\nconsole.log(1);\n";
  const program = parseProgram("javascript", src);
  assert(
    removeImport(program, { type: "default", from: "mod" }) === null,
    "Without removeSideEffectForms, bare require should not be removed (backward compatible)",
  );
  const edit = removeImport(program, {
    type: "default",
    from: "mod",
    removeSideEffectForms: true,
  });
  assert(edit !== null, "With removeSideEffectForms, bare require should be removed");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("require('mod')"), "Should strip bare require");
  assert(result.includes("console.log(1)"), "Should keep other statements");
}

/** Do not strip `require` when the module id only appears nested (not the direct specifier). */
function testRemoveBareRequireNestedStringNotRemoved() {
  const src = "require(getName('mod'));\nconsole.log(1);\n";
  const program = parseProgram("javascript", src);
  assert(
    removeImport(program, {
      type: "default",
      from: "mod",
      removeSideEffectForms: true,
    }) === null,
    "Nested string literal must not be treated as require('mod')",
  );
  assert(program.text() === src, "Source must be unchanged");
}

/** Parenthesized string literal is still a direct specifier. */
function testRemoveBareRequireParenthesizedLiteralStillRemoved() {
  const src = "require(('mod'));\nconsole.log(1);\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, {
    type: "default",
    from: "mod",
    removeSideEffectForms: true,
  });
  assert(edit !== null, "Parenthesized literal should still count as direct specifier");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("require"), "Should strip bare require");
  assert(result.includes("console.log(1)"), "Should keep other statements");
}

/** Side-effect `import 'mod'` — same as bare require: needs removeSideEffectForms. */
function testRemoveSideEffectImportWithFlag() {
  const src = "import 'mod';\nconsole.log(1);\n";
  const program = parseProgram("javascript", src);
  assert(
    removeImport(program, { type: "default", from: "mod" }) === null,
    "Without flag, side-effect import should not be removed",
  );
  const edit = removeImport(program, {
    type: "default",
    from: "mod",
    removeSideEffectForms: true,
  });
  assert(edit !== null, "With removeSideEffectForms, side-effect import should be removed");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("import 'mod'"), "Should strip side-effect import");
  assert(result.includes("console.log(1)"), "Should keep other statements");
}

/** Only the module source string counts — not other string literals (e.g. import attributes). */
function testRemoveSideEffectImportOnlyMatchesSourceField() {
  const src = "import 'foo' assert { type: 'mod' };\nconsole.log(1);\n";
  const program = parseProgram("typescript", src);
  assert(
    removeImport(program, {
      type: "default",
      from: "mod",
      removeSideEffectForms: true,
    }) === null,
    "Package name only in import attributes must not remove the statement",
  );
  assert(program.text() === src, "Source must be unchanged");
}

// ============================================================================
// === removeImport edge cases ===
// ============================================================================

// --- Bug #1 regression: removing the only named specifier when a default
// binding is also present must keep the default intact.
function testRemoveNamed_LastNamed_KeepsDefault_ESM() {
  const program = parseProgram(
    "javascript",
    "import foo, { bar } from 'mod';\nconsole.log(foo, bar);\n",
  );
  const edit = removeImport(program, { type: "named", specifiers: ["bar"], from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result === "import foo from 'mod';\nconsole.log(foo, bar);\n",
    `Should strip only ", { bar }" and keep default "foo". Got: ${JSON.stringify(result)}`,
  );
}

// --- Bug #2 regression: default + named, but remove the only named specifier
// when there are multiple named siblings listed (still drops only the ones
// that exist in this statement).
function testRemoveNamed_LastNamed_KeepsDefault_MultipleSpecifiersArg() {
  const program = parseProgram(
    "javascript",
    "import foo, { bar } from 'mod';\nconsole.log(foo, bar);\n",
  );
  // Caller lists more specifiers than actually exist on the statement; must
  // still not drop the default binding.
  const edit = removeImport(program, {
    type: "named",
    specifiers: ["bar", "unrelated"],
    from: "mod",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result === "import foo from 'mod';\nconsole.log(foo, bar);\n",
    `Default must survive when the caller over-lists specifiers. Got: ${JSON.stringify(result)}`,
  );
}

// --- Bug #2 regression (TypeScript parse path).
function testRemoveNamed_LastNamed_KeepsDefault_TSX() {
  const program = parseProgram("tsx", "import foo, { bar } from 'mod';\nconsole.log(foo, bar);\n");
  const edit = removeImport(program, { type: "named", specifiers: ["bar"], from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result === "import foo from 'mod';\nconsole.log(foo, bar);\n",
    `TSX: should strip only ", { bar }" and keep default "foo". Got: ${JSON.stringify(result)}`,
  );
}

// --- Symmetric bug: removing the default binding when named specifiers are
// also present must keep the named imports intact.
function testRemoveDefault_KeepsNamed_ESM() {
  const program = parseProgram(
    "javascript",
    "import foo, { bar } from 'mod';\nconsole.log(foo, bar);\n",
  );
  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result === "import { bar } from 'mod';\nconsole.log(foo, bar);\n",
    `Should strip only "foo, " and keep named import for bar. Got: ${JSON.stringify(result)}`,
  );
}

// --- Default + multiple named specifiers: removing default keeps all named.
function testRemoveDefault_KeepsNamed_MultipleSpecifiers() {
  const program = parseProgram(
    "javascript",
    "import foo, { bar, baz } from 'mod';\nconsole.log(foo, bar, baz);\n",
  );
  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result === "import { bar, baz } from 'mod';\nconsole.log(foo, bar, baz);\n",
    `Got: ${JSON.stringify(result)}`,
  );
}

// --- Namespace-only statement: removeImport default must still return null
// (nothing to remove, namespace alone shouldn't be deleted via default-type).
function testRemoveDefault_NamespaceOnly_ReturnsNull() {
  const src = "import * as ns from 'mod';\nconsole.log(ns);\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(edit === null, "Default removal on namespace-only import must be null");
  assert(program.text() === src, "Source must be unchanged");
}

// --- Multi-specifier removal: listing every specifier that exists in the
// statement removes the whole statement.
function testRemoveNamed_AllSpecifiersListed_RemovesWholeStatement() {
  const program = parseProgram(
    "javascript",
    "import { foo, bar } from 'mod';\nconsole.log(foo, bar);\n",
  );
  const edit = removeImport(program, {
    type: "named",
    specifiers: ["foo", "bar"],
    from: "mod",
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result === "console.log(foo, bar);\n",
    `All specifiers listed → statement removed. Got: ${JSON.stringify(result)}`,
  );
}

// --- Multi-line named imports: removing a middle specifier must not leave
// dangling commas or blank lines, and must keep the other specifiers.
function testRemoveNamed_MultiLine_RemoveMiddle() {
  const src = "import {\n  foo,\n  bar,\n  baz,\n} from 'mod';\nconsole.log(foo, bar, baz);\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, { type: "named", specifiers: ["bar"], from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(!result.includes(",,"), `Should not produce double commas: ${JSON.stringify(result)}`);
  const importPortion = result.slice(0, result.indexOf("from 'mod'") + "from 'mod';".length);
  assert(
    !/\bbar\b/.test(importPortion),
    `bar must be gone from import. Got: ${JSON.stringify(importPortion)}`,
  );
  assert(/\bfoo\b/.test(importPortion) && /\bbaz\b/.test(importPortion), "foo and baz must remain");
  assert(result.includes("from 'mod'"), "Must still reference module");
}

// --- Multi-line named imports: removing the last specifier keeps earlier
// trailing-comma formatting clean.
function testRemoveNamed_MultiLine_RemoveLast() {
  const src = "import {\n  foo,\n  bar,\n  baz,\n} from 'mod';\nconsole.log(foo, bar, baz);\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, { type: "named", specifiers: ["baz"], from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(!result.includes(",,"), "No double commas");
  const importPortion = result.slice(0, result.indexOf("from 'mod'") + "from 'mod';".length);
  assert(
    !/\bbaz\b/.test(importPortion),
    `baz must be gone from the import. Got: ${JSON.stringify(importPortion)}`,
  );
  assert(/\bfoo\b/.test(importPortion) && /\bbar\b/.test(importPortion), "foo and bar must remain");
}

// --- `import { foo as bar }` + remove named 'foo' → match by original name,
// statement removed because it's the only specifier.
function testRemoveNamed_MatchesByOriginalName_StatementRemoved() {
  const program = parseProgram(
    "javascript",
    "import { foo as bar } from 'mod';\nconsole.log(bar);\n",
  );
  const edit = removeImport(program, { type: "named", specifiers: ["foo"], from: "mod" });
  assert(edit !== null, "Should match by original name");
  const result = program.commitEdits([edit!]);
  assert(
    !result.includes("import"),
    `Whole statement should be removed: ${JSON.stringify(result)}`,
  );
}

// --- `import { foo as bar }` + remove named 'bar' → alias is not a specifier
// name, so nothing matches, returns null.
function testRemoveNamed_AliasNotMatched_ReturnsNull() {
  const src = "import { foo as bar } from 'mod';\nconsole.log(bar);\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, { type: "named", specifiers: ["bar"], from: "mod" });
  assert(edit === null, "Alias name must not match — original name is the specifier");
  assert(program.text() === src, "Source must be unchanged");
}

// --- CJS destructured: remove one of two destructured specifiers.
function testRemoveNamed_CJS_Destructured_RemoveOne() {
  const program = parseProgram(
    "javascript",
    "const { a, b } = require('mod');\nconsole.log(a, b);\n",
  );
  const edit = removeImport(program, { type: "named", specifiers: ["a"], from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    /const\s*\{\s*b\s*\}\s*=\s*require\('mod'\);/.test(result),
    `Should leave { b } = require('mod'). Got: ${JSON.stringify(result)}`,
  );
}

// --- CJS destructured: removing the only remaining specifier drops the
// whole declaration.
function testRemoveNamed_CJS_Destructured_RemoveLast() {
  const program = parseProgram("javascript", "const { a } = require('mod');\nconsole.log(a);\n");
  const edit = removeImport(program, { type: "named", specifiers: ["a"], from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("require"), "Whole declaration should be gone");
}

// --- CJS pair pattern: removing by original key removes the whole statement
// when it's the only specifier.
function testRemoveNamed_CJS_PairPattern_RemoveByOriginal() {
  const program = parseProgram(
    "javascript",
    "const { a: aa } = require('mod');\nconsole.log(aa);\n",
  );
  const edit = removeImport(program, { type: "named", specifiers: ["a"], from: "mod" });
  assert(edit !== null, "Should match pair_pattern by original key");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("require"), "Whole declaration should be gone");
}

// --- Re-exports (pin behavior: all variants return null since removeImport
// operates on import-like statements only).
function testRemoveNamed_ExportFrom_ReturnsNull() {
  const src = "export { foo } from 'mod';\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, { type: "named", specifiers: ["foo"], from: "mod" });
  assert(edit === null, "export { foo } from 'mod' is not removable via removeImport");
  assert(program.text() === src, "Source must be unchanged");
}

function testRemoveDefault_ExportStar_ReturnsNull() {
  const src = "export * from 'mod';\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(edit === null, "export * from 'mod' is not a default import");
  assert(program.text() === src, "Source must be unchanged");
}

function testRemoveNamespace_ExportNamespace_ReturnsNull() {
  const src = "export * as ns from 'mod';\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, { type: "namespace", from: "mod" });
  assert(edit === null, "export * as ns from 'mod' is not a namespace import");
  assert(program.text() === src, "Source must be unchanged");
}

// --- CRLF line endings: removal must stay consistent and not leave dangling
// \r characters.
function testRemoveDefault_CRLF() {
  const src = "import foo from 'mod';\r\nconsole.log(foo);\r\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, { type: "default", from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result === "console.log(foo);\r\n",
    `CRLF source should collapse cleanly. Got: ${JSON.stringify(result)}`,
  );
}

function testRemoveNamed_CRLF_KeepsDefault() {
  const src = "import foo, { bar } from 'mod';\r\nconsole.log(foo, bar);\r\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, { type: "named", specifiers: ["bar"], from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    result === "import foo from 'mod';\r\nconsole.log(foo, bar);\r\n",
    `CRLF + default preserved. Got: ${JSON.stringify(result)}`,
  );
}

// --- removeSideEffectForms: trailing comment on same line must not be
// removed along with the side-effect import.
function testRemoveSideEffectImport_TrailingComment_NotRemoved() {
  const src = "import 'mod'; // keep me\nconsole.log(1);\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, {
    type: "default",
    from: "mod",
    removeSideEffectForms: true,
  });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(!result.includes("import 'mod'"), "The import must be gone");
  assert(
    result.includes("// keep me"),
    `Trailing comment must survive. Got: ${JSON.stringify(result)}`,
  );
}

// --- Two side-effect imports of same module: only one edit returned.
function testRemoveSideEffectImport_TwoCopies_OnlyOneEdit() {
  const src = "import 'mod';\nimport 'mod';\nconsole.log(1);\n";
  const program = parseProgram("javascript", src);
  const edit = removeImport(program, {
    type: "default",
    from: "mod",
    removeSideEffectForms: true,
  });
  assert(edit !== null, "First call should return an edit");
  const afterFirst = program.commitEdits([edit!]);
  assert(
    (afterFirst.match(/import 'mod';/g) || []).length === 1,
    "Only one of the two side-effect imports should be removed on first call",
  );
  // Subsequent call on a fresh parse removes the next.
  const program2 = parseProgram("javascript", afterFirst);
  const edit2 = removeImport(program2, {
    type: "default",
    from: "mod",
    removeSideEffectForms: true,
  });
  assert(edit2 !== null, "Second call should remove the remaining one");
  const afterSecond = program2.commitEdits([edit2!]);
  assert(!afterSecond.includes("import 'mod'"), "Both copies should now be gone");
}

// --- TS type-only import: removing the only type specifier removes the
// entire `import type` statement.
function testRemoveNamed_TSTypeOnly_StatementRemoved() {
  const program = parseProgram("typescript", "import type { X } from 'mod';\ntype Y = X;\n");
  const edit = removeImport(program, { type: "named", specifiers: ["X"], from: "mod" });
  assert(edit !== null, "Should return an edit for `import type { X }`");
  const result = program.commitEdits([edit!]);
  assert(
    !result.includes("import type"),
    `Whole statement should be removed. Got: ${JSON.stringify(result)}`,
  );
  assert(result.includes("type Y"), "Unrelated code kept");
}

// --- TS inline type specifier: removing one keeps the other; the `type`
// keyword on the removed specifier goes away with it.
function testRemoveNamed_TSInlineType_Other_Remains() {
  const program = parseProgram("typescript", "import { type X, y } from 'mod';\nconsole.log(y);\n");
  const edit = removeImport(program, { type: "named", specifiers: ["X"], from: "mod" });
  assert(edit !== null, "Should return an edit");
  const result = program.commitEdits([edit!]);
  assert(
    /import\s*\{\s*y\s*\}\s*from\s*'mod'/.test(result),
    `Should leave import { y } from 'mod'. Got: ${JSON.stringify(result)}`,
  );
  assert(!result.includes("type X"), "Removed inline type specifier must be gone");
}

function run() {
  // getAllImports tests
  testReturnsEmptyArrayWhenNoImports();
  testReturnsEmptyArrayWhenModuleNotImported();
  testReturnsEmptyArrayWhenNamedSpecifierNotFound();
  testSingleDefaultESMImport();
  testSingleDefaultCJSImport();
  testSingleNamedImportWithAlias();
  testSingleNamedImportWithoutAlias();
  testSingleDestructuredCJSImport();
  testMultipleDefaultImports_ESMandCJS();
  testMultipleNamedImports_ESMandCJS();
  testMultipleNamedImports_SameModuleDifferentAliases();
  testMultipleDefaultImports_OnlyReturnsMatchingModule();
  testNamespaceFallback_WhenNoTypedMatchFound_DefaultQuery();
  testNamespaceFallback_WhenNoTypedMatchFound_NamedQuery();
  testNamespaceNotReturnedWhenTypedResultsExist();
  testSingleNamespaceImport_getAllImports_StillWorks();
  testMultipleNamespaceImports_getAllImports_AllReturned();
  testMultipleNamespaceImports_NamedQuery_getAllImports_AllReturned();
  testNamespaceNotReturnedAlongsideTypedResults_getAllImports();

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
  testAddImportAfterExistingKeepsSeparateLines();
  testAddImportAfterMixedImportsUsesLastSourcePosition();

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
  testRemoveDefaultVarCJS();
  testRemoveBareRequireOnlyWithSideEffectFlag();
  testRemoveBareRequireNestedStringNotRemoved();
  testRemoveBareRequireParenthesizedLiteralStillRemoved();
  testRemoveSideEffectImportWithFlag();
  testRemoveSideEffectImportOnlyMatchesSourceField();

  // removeImport edge cases
  testRemoveNamed_LastNamed_KeepsDefault_ESM();
  testRemoveNamed_LastNamed_KeepsDefault_MultipleSpecifiersArg();
  testRemoveNamed_LastNamed_KeepsDefault_TSX();
  testRemoveDefault_KeepsNamed_ESM();
  testRemoveDefault_KeepsNamed_MultipleSpecifiers();
  testRemoveDefault_NamespaceOnly_ReturnsNull();
  testRemoveNamed_AllSpecifiersListed_RemovesWholeStatement();
  testRemoveNamed_MultiLine_RemoveMiddle();
  testRemoveNamed_MultiLine_RemoveLast();
  testRemoveNamed_MatchesByOriginalName_StatementRemoved();
  testRemoveNamed_AliasNotMatched_ReturnsNull();
  testRemoveNamed_CJS_Destructured_RemoveOne();
  testRemoveNamed_CJS_Destructured_RemoveLast();
  testRemoveNamed_CJS_PairPattern_RemoveByOriginal();
  testRemoveNamed_ExportFrom_ReturnsNull();
  testRemoveDefault_ExportStar_ReturnsNull();
  testRemoveNamespace_ExportNamespace_ReturnsNull();
  testRemoveDefault_CRLF();
  testRemoveNamed_CRLF_KeepsDefault();
  testRemoveSideEffectImport_TrailingComment_NotRemoved();
  testRemoveSideEffectImport_TwoCopies_OnlyOneEdit();
  testRemoveNamed_TSTypeOnly_StatementRemoved();
  testRemoveNamed_TSInlineType_Other_Remains();

  console.log("imports.test.ts: all assertions passed");
}

try {
  run();
} catch (error) {
  console.error(error);
  process.exit(1);
}

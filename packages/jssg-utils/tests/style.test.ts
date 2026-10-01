import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import { getFileStyle, getLineIndent, indentText } from "../src/javascript/exports/style.ts";

function parseProgram(src: string) {
  return parse<TS>("typescript", src).root();
}

function testFileWithoutSpecifierUsesItsOwnStatements() {
  const bare = getFileStyle(parseProgram("const x = 1\n"));
  assert(bare.quote === "'", "A file with no string keeps single quotes");
  assert(bare.semicolon === "", "A statement that omits the semicolon is sampled");
  assert(bare.indentUnit === "  ", "A file with no block uses two spaces");

  const styled = getFileStyle(
    parseProgram('const msg = "hello"\n\nexport function f() {\n  return 1\n}\n'),
  );
  assert(styled.quote === '"', "The first string literal provides the quote");
  assert(styled.semicolon === "", "The first semicolon-style statement provides the semicolon");

  const withSemicolon = getFileStyle(parseProgram("const x = 1;\n"));
  assert(withSemicolon.quote === "'", "No string still keeps single quotes");
  assert(withSemicolon.semicolon === ";", "A statement that ends with a semicolon is sampled");
}

function testNothingToSampleKeepsFallbacks() {
  const style = getFileStyle(parseProgram("function f() {}\n"));

  assert(style.quote === "'", "A file with no string keeps single quotes");
  assert(style.semicolon === ";", "A function declaration does not choose semicolon style");
}

function testFirstStringWinsWhenThereIsNoSpecifier() {
  const style = getFileStyle(parseProgram("const a = \"one\"\nconst b = 'two';\n"));

  assert(style.quote === '"', "The first string literal wins");
  assert(style.semicolon === "", "The first statement wins over a later semicolon");
}

function testReturnAndExportClauseSetSemicolon() {
  const returned = getFileStyle(parseProgram("function f() {\n  return 1;\n}\n"));
  assert(returned.semicolon === ";", "A return statement inside a function is sampled");

  const clause = getFileStyle(parseProgram("export { a }\n"));
  assert(clause.quote === "'", "An export clause with no string keeps single quotes");
  assert(clause.semicolon === "", "An export clause can omit the semicolon");

  const typeAlias = getFileStyle(parseProgram('type Foo = "bar"\n'));
  assert(typeAlias.quote === '"', "A type alias string provides the quote");
  assert(typeAlias.semicolon === "", "A type alias can omit the semicolon");
}

function testReexportSpecifierWinsOverLaterCode() {
  const style = getFileStyle(parseProgram("const name = 'local'\nexport { a } from \"mod\";\n"));

  assert(style.quote === '"', "The re-export specifier wins over an earlier string");
  assert(style.semicolon === ";", "The re-export statement wins over an earlier statement");
}

function testFirstImportWinsOverLaterQuotes() {
  const program = parseProgram("import a from \"first\"\nimport b from 'second';\n");
  const style = getFileStyle(program);

  assert(style.quote === '"', "The first import's quote wins");
  assert(style.semicolon === "", "The first import's missing semicolon wins");
}

function testTypeOnlyAndSideEffectImportsStillSetStyle() {
  const typeOnly = getFileStyle(parseProgram('import type { Foo } from "mod";\n'));
  assert(typeOnly.quote === '"', "A type-only import still provides the quote");
  assert(typeOnly.semicolon === ";", "A type-only import still provides the semicolon");

  const sideEffect = getFileStyle(parseProgram("import 'zone.js'\n"));
  assert(sideEffect.quote === "'", "A side-effect import still provides the quote");
  assert(sideEffect.semicolon === "", "A side-effect import can omit the semicolon");
}

function testRequireCallSetsStyleWhenThereIsNoImportStatement() {
  const program = parseProgram('const fs = require("node:fs");\n');
  const style = getFileStyle(program);

  assert(style.quote === '"', "A require() call provides the quote");
  assert(style.semicolon === ";", "A require() call provides the semicolon");
}

function testIndentUnitSkipsLeadingCommentAndKeepsTabs() {
  const spaces = getFileStyle(parseProgram("function f() {\n// note\n    return 1;\n}\n"));
  assert(spaces.indentUnit === "    ", "A column-zero comment is not the indent sample");

  const tabs = getFileStyle(parseProgram("function f() {\n\treturn 1;\n}\n"));
  assert(tabs.indentUnit === "\t", "A tab-indented block keeps the tab");
}

function testLineIndentIgnoresMidLineNodesAndCarriageReturn() {
  const call = parseProgram("foo(bar);\n");
  const bar = call.findAll({ rule: { kind: "identifier" } }).find((node) => node.text() === "bar");
  assert(bar !== undefined, "Should find the argument identifier");
  assert(getLineIndent(call.text(), bar!) === "", "A mid-line node has no indent");

  const indented = parseProgram("  foo(bar);\n");
  const indentedBar = indented
    .findAll({ rule: { kind: "identifier" } })
    .find((node) => node.text() === "bar");
  assert(indentedBar !== undefined, "Should find the indented argument");
  assert(
    getLineIndent(indented.text(), indentedBar!) === "",
    "Leading whitespace does not indent a mid-line node",
  );

  const source = "function f() {\r\n  return 1;\r\n}\r\n";
  const block = parseProgram(source);
  const statement = block.find({ rule: { kind: "return_statement" } });
  assert(statement !== null, "Should find the return");
  assert(getLineIndent(source, statement!) === "  ", "CRLF indent is spaces only");
}

function testLineIndentUsesByteOffsetAfterNonAscii() {
  const source = "const café = 1;\n  return 1;\n";
  const program = parseProgram(source);
  const statement = program.find({ rule: { kind: "return_statement" } });
  assert(statement !== null, "Should find the return");
  assert(
    getLineIndent(program.text(), statement!) === "  ",
    "Non-ASCII text does not shift the indent",
  );
}

function testIndentTextLeavesBlankLinesBlank() {
  assert(indentText("a\n\nb\n", "  ") === "  a\n\n  b\n", "Blank lines are not indented");
}

function run() {
  testFileWithoutSpecifierUsesItsOwnStatements();
  testNothingToSampleKeepsFallbacks();
  testFirstStringWinsWhenThereIsNoSpecifier();
  testReturnAndExportClauseSetSemicolon();
  testReexportSpecifierWinsOverLaterCode();
  testFirstImportWinsOverLaterQuotes();
  testTypeOnlyAndSideEffectImportsStillSetStyle();
  testRequireCallSetsStyleWhenThereIsNoImportStatement();
  testIndentUnitSkipsLeadingCommentAndKeepsTabs();
  testLineIndentIgnoresMidLineNodesAndCarriageReturn();
  testLineIndentUsesByteOffsetAfterNonAscii();
  testIndentTextLeavesBlankLinesBlank();
  console.log("style.test.ts: all assertions passed");
}

try {
  run();
} catch (error) {
  console.error(error);
  process.exit(1);
}

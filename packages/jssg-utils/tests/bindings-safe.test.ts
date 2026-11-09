import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import {
  importBindingOf,
  renameReferences,
  resolveDefinition,
  resolvesToFactory,
} from "../src/javascript/exports/bindings.ts";

function testMissingProviderDoesNotThrow() {
  const program = parse<TS>("typescript", 'import { setup } from "lib";\nsetup();\n').root();
  const call = program.find({ rule: { kind: "call_expression" } });
  assert(call !== null, "Should find the call");
  const callee = call!.field("function");
  assert(callee !== null, "Should find the callee");

  assert(importBindingOf(callee!) === null, "No provider means no binding");
  assert(resolveDefinition(callee!) === null, "No provider means no definition");
  assert(resolvesToFactory(callee!, "setup", ["lib"]) === false, "No provider is not a factory");
  assert(
    renameReferences(callee!, "next", "anonymous").length === 0,
    "No provider means no rename edits",
  );
}

try {
  testMissingProviderDoesNotThrow();
  console.log("bindings-safe.test.ts: all assertions passed");
} catch (error) {
  console.error(error);
  process.exit(1);
}

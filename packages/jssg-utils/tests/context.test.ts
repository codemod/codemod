import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import type JS from "@codemod.com/jssg-types/langs/javascript";
import type TSX from "@codemod.com/jssg-types/langs/tsx";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import type { SgNode } from "@codemod.com/jssg-types/main";
import {
  getNamedChildren,
  isUsedAsConstructor,
  isUsedInReflectiveAccess,
  unwrapParenthesizedExpression,
} from "../src/javascript/exports/context.ts";

type Language = JS | TS | TSX;

function parseProgram<T extends Language>(lang: string, src: string) {
  return parse<T>(lang, src).root();
}

function requireNode<T>(node: T | null | undefined, message: string): T {
  assert(node != null, message);
  return node;
}

function findIdentifierWithAncestorKind(
  program: SgNode<Language, "program">,
  name: string,
  ancestorKind: string,
) {
  return (
    program
      .findAll({
        rule: {
          kind: "identifier",
          pattern: name,
        },
      })
      .find((node) =>
        node.ancestors().some((ancestor) => String(ancestor.kind()) === ancestorKind),
      ) ?? null
  );
}

function testIsUsedAsConstructorThroughTransparentWrappers() {
  const program = parseProgram(
    "javascript",
    "const boundFn = fn.bind(obj); new ((0, boundFn))();\n",
  );

  const usage = requireNode(
    findIdentifierWithAncestorKind(program, "boundFn", "sequence_expression"),
    "Should find constructor-wrapped boundFn identifier",
  );
  assert(isUsedAsConstructor(usage), "Wrapped constructor usage should be detected");
}

function testIsUsedInReflectiveAccessHandlesComputedAndInOperator() {
  const computedProgram = parseProgram(
    "javascript",
    'const boundFn = fn.bind(obj); (0, boundFn)["name"];\n',
  );
  const computedUsage = requireNode(
    findIdentifierWithAncestorKind(computedProgram, "boundFn", "sequence_expression"),
    "Should find computed reflective usage",
  );
  assert(
    isUsedInReflectiveAccess(computedUsage, ["name", "length", "prototype"]),
    "Computed reflective access should be detected",
  );

  const inProgram = parseProgram(
    "javascript",
    'const boundFn = fn.bind(obj); "prototype" in (0, boundFn);\n',
  );
  const inUsage = requireNode(
    findIdentifierWithAncestorKind(inProgram, "boundFn", "sequence_expression"),
    "Should find in-operator reflective usage",
  );
  assert(
    isUsedInReflectiveAccess(inUsage, ["name", "length", "prototype"]),
    "Reflective in-operator usage should be detected",
  );

  const leftShortCircuitProgram = parseProgram(
    "javascript",
    'const boundFn = fn.bind(obj); (boundFn || fallback)["name"];\n',
  );
  const leftShortCircuitUsage = requireNode(
    leftShortCircuitProgram.find({
      rule: {
        kind: "identifier",
        pattern: "boundFn",
      },
    }),
    "Should find left short-circuit operand usage",
  );
  assert(
    !isUsedInReflectiveAccess(leftShortCircuitUsage, ["name", "length", "prototype"]),
    "Left short-circuit operands should not bubble into reflective access detection",
  );
}

function testUnwrapParenthesizedExpressionOnlyStripsParens() {
  const program = parseProgram("javascript", "const value = (((nested ? a : b)));\n");

  const ternary = requireNode(
    program.find({
      rule: {
        kind: "ternary_expression",
      },
    }),
    "Should find parenthesized ternary expression",
  );

  const wrapped = requireNode(
    ternary.parent()?.parent()?.parent(),
    "Should find outer parenthesized wrapper",
  );
  const unwrapped = unwrapParenthesizedExpression(wrapped);
  assert(
    unwrapped.id() === ternary.id(),
    "Should unwrap nested parentheses to the inner expression",
  );
}

function testGetNamedChildrenSkipsTokensAndComments() {
  const program = parseProgram("javascript", "if (cond) { /* keep hidden */ value = one; }\n");

  const block = requireNode(
    program.find({
      rule: {
        kind: "statement_block",
      },
    }),
    "Should find statement block",
  );

  const children = getNamedChildren(block);
  assert(children.length === 1, "Should return only named, non-comment children");
  assert(children[0]?.kind() === "expression_statement", "Should keep the statement child");
}

testGetNamedChildrenSkipsTokensAndComments();
testIsUsedAsConstructorThroughTransparentWrappers();
testIsUsedInReflectiveAccessHandlesComputedAndInOperator();
testUnwrapParenthesizedExpressionOnlyStripsParens();
console.log("context.test.ts: all assertions passed");

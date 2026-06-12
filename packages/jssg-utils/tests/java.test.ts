import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import type Java from "@codemod.com/jssg-types/langs/java";
import type { SgNode } from "@codemod.com/jssg-types/main";
import {
  cleanupJavaImports,
  collectJavaImports,
  hasConflictingJavaSimpleImport,
  isJavaTypeImported,
} from "@jssg/utils/java/imports";
import { findVisibleJavaDeclarationBeforeUsage } from "@jssg/utils/java/scope";
import { replaceJavaTypeIdentifierSafely } from "@jssg/utils/java/types";
import {
  getJavaMethodInvocationParts,
  getJavaReceiverIdentifier,
} from "@jssg/utils/java/method-invocations";
import { rewriteAnonymousJavaCallbackToWhenComplete } from "@jssg/utils/java/callbacks";

type JavaNode = SgNode<Java>;

function parseJava(source: string): JavaNode {
  return parse<Java>("java", source).root();
}

function testCollectJavaImportsHandlesWildcardsAndConflicts() {
  const root = parseJava(
    [
      "import org.springframework.http.*;",
      "import com.example.ResponseEntity;",
      "",
      "class Example {}",
    ].join("\n"),
  );
  const imports = collectJavaImports(root);

  assert(
    isJavaTypeImported(imports, {
      simpleName: "ResponseEntity",
      fullyQualifiedName: "org.springframework.http.ResponseEntity",
    }),
    "Wildcard Spring HTTP import should import ResponseEntity",
  );
  assert(
    hasConflictingJavaSimpleImport(imports, {
      simpleName: "ResponseEntity",
      expectedFullyQualifiedName: "org.springframework.http.ResponseEntity",
    }),
    "Non-Spring exact ResponseEntity import should be reported as conflicting",
  );
}

function testCleanupJavaImportsRemovesOldSpringImportsAndAddsNewImport() {
  const input = [
    "import org.springframework.scheduling.annotation.AsyncResult;",
    "import org.springframework.util.concurrent.ListenableFuture;",
    "import org.springframework.util.concurrent.SettableListenableFuture;",
    "",
    "class Example {",
    "  CompletableFuture<String> work(CompletableFuture<String> input) {",
    '    return CompletableFuture.completedFuture("done");',
    "  }",
    "}",
    "",
  ].join("\n");

  const output = cleanupJavaImports(input, {
    removeIfUnreferenced: [
      "org.springframework.scheduling.annotation.AsyncResult",
      "org.springframework.util.concurrent.ListenableFuture",
      "org.springframework.util.concurrent.SettableListenableFuture",
    ],
    addIfReferenced: ["java.util.concurrent.CompletableFuture"],
  });

  assert(
    output ===
      [
        "import java.util.concurrent.CompletableFuture;",
        "",
        "class Example {",
        "  CompletableFuture<String> work(CompletableFuture<String> input) {",
        '    return CompletableFuture.completedFuture("done");',
        "  }",
        "}",
        "",
      ].join("\n"),
    "Should remove obsolete Spring imports and add CompletableFuture",
  );
}

function testFindVisibleDeclarationBeforeUsagePrefersPriorLocalOverField() {
  const root = parseJava(
    [
      "import org.springframework.util.concurrent.ListenableFuture;",
      "",
      "class Example {",
      "  private ListenableFuture<String> future;",
      "",
      "  void wire() {",
      "    com.google.common.util.concurrent.ListenableFuture<String> future = getGuavaFuture();",
      "    future.addCallback(this::handle, this::recover);",
      "  }",
      "}",
    ].join("\n"),
  );
  const invocation = root.find({ rule: { pattern: "$RECEIVER.addCallback($A, $B)" } });
  assert(invocation !== null, "Should find addCallback invocation");

  const parts = getJavaMethodInvocationParts(invocation!);
  assert(parts?.receiver !== null, "Should read receiver");

  const declaration = findVisibleJavaDeclarationBeforeUsage({
    usageNode: invocation!,
    name: getJavaReceiverIdentifier(parts!.receiver)!,
  });

  assert(declaration !== null, "Should resolve visible declaration");
  assert(declaration!.kind === "local", "Local declaration should shadow the field");
  assert(
    declaration!.typeText === "com.google.common.util.concurrent.ListenableFuture<String>",
    "Should return the Guava local type, not the Spring field type",
  );
}

function testReplaceJavaTypeIdentifierSafelySkipsFqcnSubnode() {
  const root = parseJava(
    [
      "class Example {",
      "  com.google.common.util.concurrent.ListenableFuture<String> getGuavaFuture() {",
      "    return null;",
      "  }",
      "}",
    ].join("\n"),
  );
  const simpleType = root
    .findAll({ rule: { kind: "type_identifier" } })
    .find((node) => node.text() === "ListenableFuture");
  assert(simpleType !== null, "Should find nested simple type identifier");

  const edit = replaceJavaTypeIdentifierSafely(simpleType!, "CompletableFuture");
  assert(edit === null, "Should skip simple type identifiers inside FQCN scoped types");
}

function testReplaceJavaTypeIdentifierSafelyReplacesGenericType() {
  const root = parseJava(
    [
      "class Example {",
      "  ListenableFuture<String> getFuture() {",
      "    return null;",
      "  }",
      "}",
    ].join("\n"),
  );
  const genericType = root.find({
    rule: { kind: "generic_type", pattern: "ListenableFuture<String>" },
  });
  assert(genericType !== null, "Should find generic type");

  const edit = replaceJavaTypeIdentifierSafely(genericType!, "CompletableFuture");
  assert(edit !== null, "Should create an edit for simple generic type");
  assert(
    root.commitEdits([edit!]).includes("CompletableFuture<String> getFuture()"),
    "Should preserve generic type arguments",
  );
}

function testRewriteAnonymousJavaCallbackToWhenCompleteRenamesDuplicateParams() {
  const root = parseJava(
    [
      "import org.springframework.util.concurrent.ListenableFuture;",
      "import org.springframework.util.concurrent.ListenableFutureCallback;",
      "",
      "class Example {",
      "  void wire(ListenableFuture<String> future) {",
      "    future.addCallback(new ListenableFutureCallback<String>() {",
      "      @Override",
      "      public void onSuccess(String value) {",
      "        handle(value);",
      "      }",
      "",
      "      @Override",
      "      public void onFailure(Throwable value) {",
      "        recover(value);",
      "      }",
      "    });",
      "  }",
      "}",
    ].join("\n"),
  );
  const callback = root.find({ rule: { kind: "object_creation_expression" } });
  assert(callback !== null, "Should find anonymous callback");

  const replacement = rewriteAnonymousJavaCallbackToWhenComplete({
    receiverText: "future",
    callbackNode: callback!,
  });

  assert(replacement !== null, "Should build callback replacement");
  assert(
    replacement!.includes("future.whenComplete((valueResult, value) ->"),
    "Should rename duplicate success parameter",
  );
  assert(
    replacement!.includes("handle(valueResult);"),
    "Should rewrite success body references to renamed parameter",
  );
  assert(replacement!.includes("recover(value);"), "Should preserve failure body references");
}

testCollectJavaImportsHandlesWildcardsAndConflicts();
testCleanupJavaImportsRemovesOldSpringImportsAndAddsNewImport();
testFindVisibleDeclarationBeforeUsagePrefersPriorLocalOverField();
testReplaceJavaTypeIdentifierSafelySkipsFqcnSubnode();
testReplaceJavaTypeIdentifierSafelyReplacesGenericType();
testRewriteAnonymousJavaCallbackToWhenCompleteRenamesDuplicateParams();

import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import type Java from "@codemod.com/jssg-types/langs/java";
import type { SgNode } from "@codemod.com/jssg-types/main";
import {
  cleanupImports,
  collectImports,
  hasConflictingSimpleImport,
  isTypeImported,
} from "@jssg/utils/java/imports";
import { findVisibleDeclarationBeforeUsage } from "@jssg/utils/java/scope";
import { replaceTypeIdentifierSafely } from "@jssg/utils/java/types";
import {
  getMethodInvocationParts,
  getReceiverIdentifier,
} from "@jssg/utils/java/method-invocations";
import {
  getAnonymousClassMethods,
  getMethodBodyContent,
  getSingleParameterName,
} from "@jssg/utils/java/anonymous-classes";

type JavaNode = SgNode<Java>;

function parseJava(source: string): JavaNode {
  return parse<Java>("java", source).root();
}

function testCollectImportsHandlesWildcardsAndConflicts() {
  const root = parseJava(
    [
      "import org.springframework.http.*;",
      "import com.example.ResponseEntity;",
      "",
      "class Example {}",
    ].join("\n"),
  );
  const imports = collectImports(root);

  assert(
    isTypeImported(imports, {
      simpleName: "ResponseEntity",
      fullyQualifiedName: "org.springframework.http.ResponseEntity",
    }),
    "Wildcard Spring HTTP import should import ResponseEntity",
  );
  assert(
    hasConflictingSimpleImport(imports, {
      simpleName: "ResponseEntity",
      expectedFullyQualifiedName: "org.springframework.http.ResponseEntity",
    }),
    "Non-Spring exact ResponseEntity import should be reported as conflicting",
  );
}

function testCleanupImportsRemovesOldSpringImportsAndAddsNewImport() {
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

  const output = cleanupImports(input, {
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

  const parts = getMethodInvocationParts(invocation!);
  assert(parts?.receiver !== null, "Should read receiver");

  const declaration = findVisibleDeclarationBeforeUsage({
    usageNode: invocation!,
    name: getReceiverIdentifier(parts!.receiver)!,
  });

  assert(declaration !== null, "Should resolve visible declaration");
  assert(declaration!.kind === "local", "Local declaration should shadow the field");
  assert(
    declaration!.typeText === "com.google.common.util.concurrent.ListenableFuture<String>",
    "Should return the Guava local type, not the Spring field type",
  );
}

function testReplaceTypeIdentifierSafelySkipsFqcnSubnode() {
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

  const edit = replaceTypeIdentifierSafely(simpleType!, "CompletableFuture");
  assert(edit === null, "Should skip simple type identifiers inside FQCN scoped types");
}

function testReplaceTypeIdentifierSafelyReplacesGenericType() {
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

  const edit = replaceTypeIdentifierSafely(genericType!, "CompletableFuture");
  assert(edit !== null, "Should create an edit for simple generic type");
  assert(
    root.commitEdits([edit!]).includes("CompletableFuture<String> getFuture()"),
    "Should preserve generic type arguments",
  );
}

function testAnonymousClassMethodHelpersSupportCallbackMigrationShapes() {
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

  const methods = getAnonymousClassMethods(callback!, ["onSuccess", "onFailure"]);
  assert(methods !== null, "Should find both callback methods");

  const success = methods!.get("onSuccess")!;
  const failure = methods!.get("onFailure")!;
  const successParam = getSingleParameterName(success);
  const failureParam = getSingleParameterName(failure);

  assert(successParam === "value", "Should read success parameter name");
  assert(failureParam === "value", "Should read failure parameter name");

  const resultParam = successParam === failureParam ? `${successParam}Result` : successParam!;
  const successBody = getMethodBodyContent(success, {
    from: successParam!,
    to: resultParam,
  });
  const failureBody = getMethodBodyContent(failure);

  assert(successBody !== null, "Should read success method body");
  assert(failureBody !== null, "Should read failure method body");
  assert(
    successBody!.includes("handle(valueResult);"),
    "Should allow callers to rename duplicate success parameters through AST edits",
  );
  assert(failureBody!.includes("recover(value);"), "Should preserve failure body references");
}

testCollectImportsHandlesWildcardsAndConflicts();
testCleanupImportsRemovesOldSpringImportsAndAddsNewImport();
testFindVisibleDeclarationBeforeUsagePrefersPriorLocalOverField();
testReplaceTypeIdentifierSafelySkipsFqcnSubnode();
testReplaceTypeIdentifierSafelyReplacesGenericType();
testAnonymousClassMethodHelpersSupportCallbackMigrationShapes();

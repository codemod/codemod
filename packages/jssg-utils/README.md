# @jssg/utils

Utilities used by the JSSG codemod engine.

Author-facing helpers are documented in [JSSG utilities](https://docs.codemod.com/community/jssg/utils), with one page per language under `docs/community/jssg/utils/`. Codemod MCP serves those pages as `jssg-utils-instructions`.

## JavaScript imports

```ts
import {
  getImport,
  getAllImports,
  listImports,
  addImport,
  removeImport,
  updateImport,
  stringToExactRegexString,
} from "@jssg/utils/javascript/imports";
import { getFileStyle, getLineIndent, indentText } from "@jssg/utils/javascript/style";
import {
  importBindingOf,
  resolveDefinition,
  resolvesToFactory,
  renameReferences,
} from "@jssg/utils/javascript/bindings";
```

## XML elements

```ts
import {
  findElementsByTag,
  findElementByTag,
  findElementByKind,
  getAttributeValue,
  hasTag,
  getLineIndent,
} from "@jssg/utils/xml/elements";
```

## Java

```ts
import {
  cleanupImports,
  collectImports,
  createImportCleanupEdits,
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
  getAnonymousClassMethod,
  getAnonymousClassMethods,
  getMethodBodyContent,
  getSingleParameterName,
  renameIdentifiersInNode,
} from "@jssg/utils/java/anonymous-classes";
```

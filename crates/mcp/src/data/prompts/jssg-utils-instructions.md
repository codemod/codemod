# JSSG Utils (`@jssg/utils`)

A collection of reusable utility functions for JSSG codemods. These helpers handle common import manipulation tasks that are tedious and error-prone when done manually with AST queries.

## Installation

`@jssg/utils` is available as an npm package. Add it to your codemod project:

```json
{
  "dependencies": {
    "@jssg/utils": "latest"
  }
}
```

## Import

```typescript
import { getImport, addImport, removeImport } from "@jssg/utils/javascript/imports";
```

## Supported Languages

These utilities work with JavaScript, TypeScript, JSX, and TSX ASTs.

---

## `getImport(program, options)`

Locate an import of a given module and return its alias/identifier node.

### Parameters

- `program: SgNode<T, "program">` — The root program node to search within.
- `options: GetImportOptions` — What to look for:
  - `{ type: "default", from: string }` — Find the default import from a module.
  - `{ type: "named", name: string, from: string }` — Find a specific named import from a module.

### Returns

`GetImportResult<T> | null` — An object containing:
- `alias: string` — The local name used at call sites (e.g., `baz` in `import { foo as baz }`)
- `isNamespace: boolean` — `true` if the import is `import * as xyz from '...'`
- `moduleType: "esm" | "cjs"` — Whether the import is ESM (`import`) or CJS (`require()`)
- `node: SgNode<T, "identifier">` — The identifier AST node for the local binding

Returns `null` if the import is not found.

### Supported Import Shapes

- ESM default: `import foo from "module"`
- ESM named: `import { bar } from "module"` or `import { bar as baz } from "module"`
- ESM namespace: `import * as foo from "module"`
- ESM bare: `import "module"`
- CJS default: `const foo = require("module")`
- CJS destructured: `const { bar } = require("module")` or `const { bar: baz } = require("module")`
- Dynamic import: `const foo = await import("module")`

### Example

```typescript
import type { Transform } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { getImport } from "@jssg/utils/javascript/imports";

const transform: Transform<TSX> = async (root) => {
  const rootNode = root.root();

  // Find the default import of "express"
  const expressImport = getImport(rootNode, { type: "default", from: "express" });
  if (!expressImport) return null;

  // expressImport.alias is the local name (e.g., "app" from `import app from 'express'`)
  // expressImport.moduleType tells you if it's "esm" or "cjs"
  console.log(`Express imported as: ${expressImport.alias} (${expressImport.moduleType})`);

  // Find a named import
  const useStateImport = getImport(rootNode, { type: "named", name: "useState", from: "react" });
  if (useStateImport) {
    console.log(`useState aliased as: ${useStateImport.alias}`);
  }

  return null;
};

export default transform;
```

---

## `addImport(program, options)`

Add an import to the program. Smart behavior:
- **Skips** if the import already exists
- **Merges** named specifiers into an existing import statement from the same source
- **Creates** a new import statement otherwise
- Inserts after the last existing import (or at file start if no imports exist)

### Parameters

- `program: SgNode<T, "program">` — The root program node.
- `options: AddImportOptions`:
  - `{ type: "default", name: string, from: string, moduleType?: "esm" | "cjs" }` — Add a default import.
  - `{ type: "namespace", name: string, from: string }` — Add a namespace import (`import * as name`).
  - `{ type: "named", specifiers: Array<{ name: string, alias?: string }>, from: string, moduleType?: "esm" | "cjs" }` — Add named imports.

### Returns

`Edit | null` — An edit to apply via `rootNode.commitEdits()`, or `null` if the import already exists.

### Example

```typescript
import type { Transform, Edit } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { addImport } from "@jssg/utils/javascript/imports";

const transform: Transform<TSX> = async (root) => {
  const rootNode = root.root();
  const edits: Edit[] = [];

  // Add a default import (skipped if already present)
  const defaultEdit = addImport(rootNode, {
    type: "default",
    name: "React",
    from: "react",
  });
  if (defaultEdit) edits.push(defaultEdit);

  // Add named imports (merges into existing `import { ... } from 'react'` if present)
  const namedEdit = addImport(rootNode, {
    type: "named",
    specifiers: [{ name: "useState" }, { name: "useEffect" }],
    from: "react",
  });
  if (namedEdit) edits.push(namedEdit);

  // Add a CJS require
  const cjsEdit = addImport(rootNode, {
    type: "default",
    name: "express",
    from: "express",
    moduleType: "cjs",
  });
  if (cjsEdit) edits.push(cjsEdit);

  return edits.length > 0 ? rootNode.commitEdits(edits) : null;
};

export default transform;
```

---

## `removeImport(program, options)`

Remove an import from the program. Smart behavior:
- **Default/namespace**: Removes the entire import statement.
- **Named (multiple specifiers)**: Removes only the specified specifiers, keeping the rest.
- **Named (last specifier)**: Removes the entire import statement.
- Handles both ESM and CJS formats.

### Parameters

- `program: SgNode<T, "program">` — The root program node.
- `options: RemoveImportOptions`:
  - `{ type: "default", from: string }` — Remove the default import from a module.
  - `{ type: "namespace", from: string }` — Remove the namespace import from a module.
  - `{ type: "named", specifiers: string[], from: string }` — Remove specific named imports.

### Returns

`Edit | null` — An edit to apply via `rootNode.commitEdits()`, or `null` if the import was not found.

### Example

```typescript
import type { Transform, Edit } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import { removeImport } from "@jssg/utils/javascript/imports";

const transform: Transform<TSX> = async (root) => {
  const rootNode = root.root();
  const edits: Edit[] = [];

  // Remove a named import (only removes "PropTypes", keeps other specifiers)
  const removeEdit = removeImport(rootNode, {
    type: "named",
    specifiers: ["PropTypes"],
    from: "react",
  });
  if (removeEdit) edits.push(removeEdit);

  return edits.length > 0 ? rootNode.commitEdits(edits) : null;
};

export default transform;
```

---

## `stringToExactRegexString(string)`

Escape a string for use as an exact-match regex pattern (wraps in `^...$` and escapes special characters).

```typescript
import { stringToExactRegexString } from "@jssg/utils/javascript/imports";

// Returns "^my\\.module$"
const regex = stringToExactRegexString("my.module");
```

---

## Common Patterns

### Verify Import Before Transforming

Always verify that a symbol is imported from the expected package before transforming its usage:

```typescript
const reactImport = getImport(rootNode, { type: "named", name: "useState", from: "react" });
if (!reactImport) return null; // Not from React, skip

// Now safe to transform useState usage
const alias = reactImport.alias; // Handles aliased imports like `import { useState as myState }`
```

### Replace One Import With Another

```typescript
const edits: Edit[] = [];

// Remove old import
const removeEdit = removeImport(rootNode, { type: "named", specifiers: ["oldUtil"], from: "old-pkg" });
if (removeEdit) edits.push(removeEdit);

// Add new import
const addEdit = addImport(rootNode, {
  type: "named",
  specifiers: [{ name: "newUtil" }],
  from: "new-pkg",
});
if (addEdit) edits.push(addEdit);
```

### Handle Both ESM and CJS

`getImport` automatically detects the module type. Use `moduleType` when adding imports to match the existing style:

```typescript
const existing = getImport(rootNode, { type: "default", from: "express" });
const moduleType = existing?.moduleType ?? "esm"; // Match existing style

const edit = addImport(rootNode, {
  type: "named",
  specifiers: [{ name: "Router" }],
  from: "express",
  moduleType,
});
```

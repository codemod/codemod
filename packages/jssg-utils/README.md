# @jssg/utils

Utilities used by the JSSG codemod engine.

## JavaScript import helpers

Import from:

```ts
import { getImport, addImport, removeImport } from "@jssg/utils/javascript/imports";
```

These helpers work on a `program` AST node (from `codemod:ast-grep` / `@codemod.com/jssg-types`) and return either **lookup info** (`getImport`) or a single **text edit** you can apply with `program.commitEdits([edit])`.

## JavaScript binding helpers

Import from:

```ts
import {
  getTopLevelImportBinding,
  getAllTopLevelImportBindings,
  isRuntimeImportBinding,
  isTypeOnlyImportBinding,
  findShadowingBinding,
} from "@jssg/utils/javascript/bindings";
```

These helpers are intended for conservative transform gating:

- detect whether a matched identifier really comes from a top-level import
- distinguish runtime imports from `import type` bindings
- skip rewrites when a local variable shadows an imported name
- handle both plain identifier usages and JSX tag identifiers such as `<Grid />`

Typical use case:

```ts
const rootNode = root.root();
const gridUsage = rootNode.find({ rule: { kind: "identifier", pattern: "Grid" } });

if (
  gridUsage &&
  isRuntimeImportBinding(gridUsage, {
    type: "named",
    name: "Grid",
    from: "@mui/material",
  })
) {
  // Safe to treat this usage as the runtime Grid import.
}
```

### `getTopLevelImportBinding(program, options)`

Returns the first matching top-level import binding or `null`.

The result extends the import-helper result shape with:

- **`isTypeOnly`**: `true` when the binding comes from `import type` or an inline `type` specifier

Use this when the codemod only needs one binding candidate for a module/symbol pair.

Example:

```ts
const binding = getTopLevelImportBinding(program, {
  type: "named",
  name: "Grid",
  from: "@mui/material",
});

if (binding && !binding.isTypeOnly) {
  // Safe to reason about the runtime Grid import.
}
```

### `getAllTopLevelImportBindings(program, options)`

Returns every matching top-level import binding for the query.

Use this when a file may contain:

- both type-only and runtime imports for the same symbol
- multiple import forms that all need inspection
- aliased variants that should be checked individually

Example:

```ts
const bindings = getAllTopLevelImportBindings(program, {
  type: "named",
  name: "Grid",
  from: "@mui/material",
});

const runtimeBindings = bindings.filter((binding) => !binding.isTypeOnly);
```

### `isTypeOnlyImportBinding(node)`

Returns `true` when the given import binding node belongs to:

- `import type { ... } from "mod"`
- an inline `type` specifier such as `import { type Foo } from "mod"`

Use this to keep type-only imports out of runtime transforms.

Example:

```ts
const binding = getTopLevelImportBinding(program, {
  type: "named",
  name: "Grid",
  from: "@mui/material",
});

if (binding && isTypeOnlyImportBinding(binding.node)) {
  return null;
}
```

### `findShadowingBinding(node, identifierName)`

Returns the local binding node that shadows `identifierName` for the given usage, or `null` if no local shadow exists.

The helper handles:

- local variables
- function and class names
- destructured parameters
- `catch` parameters
- hoisted `var` declarations

Use this to avoid treating a local binding as an imported symbol.

Example:

```ts
const usage = rootNode.find({ rule: { kind: "identifier", pattern: "Grid" } });

if (usage && findShadowingBinding(usage, "Grid")) {
  // This usage is shadowed locally, so skip the import-based rewrite.
}
```

JSX usage works too:

```ts
const jsxGrid = rootNode.find({
  rule: {
    kind: "identifier",
    pattern: "Grid",
    inside: { kind: "jsx_self_closing_element" },
  },
});

if (
  jsxGrid &&
  isRuntimeImportBinding(jsxGrid, {
    type: "default",
    from: "@mui/material/Grid",
  })
) {
  // Safe to treat <Grid /> as the runtime imported component.
}
```

### `isNodeBoundToIdentifier(node, identifierName)`

Returns `true` when:

- the node is an identifier with the requested text
- and there is no local shadowing binding for that identifier at the usage site

This is a small conservative guard for direct identifier checks.

### `isRuntimeImportBinding(node, options)`

Returns `true` when the given usage node resolves to a non-type-only top-level import matching the query.

Use this as the main conservative gate before rewriting runtime symbol usages.

Example:

```ts
const usage = rootNode.find({ rule: { kind: "identifier", pattern: "Grid" } });

if (
  usage &&
  isRuntimeImportBinding(usage, {
    type: "named",
    name: "Grid",
    from: "@mui/material",
  })
) {
  // Rewrite this usage as the imported runtime Grid symbol.
}
```

### `getImport(program, options)`

Finds a binding for an import and returns:

- **`alias`**: the identifier you should use at call sites (resolves `as` aliases)
- **`isNamespace`**: `true` for `import * as ns from 'mod'`
- **`moduleType`**: `'esm'` for `import ...` / `import()` and `'cjs'` for `require(...)`
- **`node`**: the underlying identifier node

Supported shapes (for a given `from`):

- ESM default: `import foo from 'mod'`
- ESM named: `import { bar as baz } from 'mod'`
- ESM namespace: `import * as ns from 'mod'`
- CJS default: `const foo = require('mod')`
- CJS destructured: `const { bar: baz } = require('mod')`
- Dynamic import (assigned): `const foo = await import('mod')`
- Dynamic import (destructured): `const { bar } = await import('mod')`

Note: **side-effect-only imports** like `import 'mod'` don’t produce a binding, so `getImport` returns `null`.

### `addImport(program, options)`

Creates an import/require edit or returns `null` if it’s already present.

Options:

- **Default**: `{ type: 'default', name, from, moduleType?: 'esm' | 'cjs' }`
- **Namespace**: `{ type: 'namespace', name, from }` (always ESM)
- **Named**: `{ type: 'named', specifiers: { name; alias? }[], from, moduleType?: 'esm' | 'cjs' }`

Behavior:

- Skips if already imported (for named imports: skips only the specifiers that already exist)
- For ESM named imports, merges new specifiers into an existing `import { ... } from 'mod'` when possible
- Inserts new imports **after the last existing import/require**, otherwise at file start

Example:

```ts
import { parse } from "codemod:ast-grep";
import type TS from "@codemod.com/jssg-types/langs/typescript";
import { addImport } from "@jssg/utils/javascript/imports";

const program = parse<TS>("typescript", "console.log('hello')\n").root();

const edit = addImport(program, {
  type: "named",
  from: "mod",
  specifiers: [{ name: "foo" }, { name: "bar", alias: "baz" }],
});

if (edit) {
  const next = program.commitEdits([edit]);
  // import { foo, bar as baz } from 'mod';
}
```

### `removeImport(program, options)`

Removes an import/require and returns an edit, or `null` if nothing matches.

Options:

- **Default**: `{ type: 'default', from }`
- **Namespace**: `{ type: 'namespace', from }`
- **Named**: `{ type: 'named', specifiers: string[], from }`

Behavior:

- Default/namespace: removes the entire statement
- Named: removes a specifier; if you’re removing the last specifier(s), removes the entire statement

Note: this function returns a **single edit**. For named removals, it removes the first matching specifier it finds unless it can remove the whole statement.

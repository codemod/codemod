# @jssg/utils

Utilities used by the JSSG codemod engine.

Public JavaScript helpers are currently grouped under:

- `@jssg/utils/javascript/imports`
- `@jssg/utils/javascript/bindings`
- `@jssg/utils/javascript/context`

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
  isRuntimeImportBinding,
} from "@jssg/utils/javascript/bindings";
import {
  getNamedChildren,
  isUsedAsConstructor,
  isUsedInReflectiveAccess,
  unwrapParenthesizedExpression,
} from "@jssg/utils/javascript/context";
```

These helpers are intended for conservative transform gating:

- detect whether a matched identifier really comes from a top-level import
- skip rewrites when a local variable shadows an imported name
- handle both plain identifier usages and JSX tag identifiers such as `<Grid />`

Typical use case:

```ts
const rootNode = root.root();
const gridUsage = rootNode.find({ rule: { kind: "identifier", pattern: "Grid" } });

if (
  gridUsage &&
  isRuntimeImportBinding(gridUsage)
) {
  // Safe to treat this usage as the runtime Grid import.
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
  isRuntimeImportBinding(jsxGrid)
) {
  // Safe to treat <Grid /> as the runtime imported component.
}
```

### `isRuntimeImportBinding(node)`

Returns `true` when the given usage node resolves to a non-type-only top-level runtime import.

Use this as the main conservative gate before rewriting runtime symbol usages.

Example:

```ts
const usage = rootNode.find({ rule: { kind: "identifier", pattern: "Grid" } });

if (usage && isRuntimeImportBinding(usage)) {
  // Rewrite this usage as the imported runtime Grid symbol.
}
```

## JavaScript context helpers

Import from:

```ts
import {
  isUsedAsConstructor,
  isUsedInReflectiveAccess,
  unwrapParenthesizedExpression,
} from "@jssg/utils/javascript/context";
```

These helpers are intended for conservative usage-context checks:

- fetch named AST children while skipping tokens and comment nodes
- strip only extra parentheses when a codemod needs expression normalization
- recognize constructor and reflective usage after wrapper expressions
- keep wrapper/context logic out of codemod-local string heuristics

Typical use case:

```ts
if (isUsedAsConstructor(bindCall)) {
  return null;
}

if (isUsedInReflectiveAccess(bindCall, ["name", "length", "prototype", "toString"])) {
  return null;
}
```

### `getNamedChildren(node)`

Returns the node's named children while skipping non-named tokens and comment nodes.

Use this when a codemod wants AST children that correspond to semantic nodes rather
than punctuation or comment trivia.

### `unwrapParenthesizedExpression(node)`

Returns the innermost expression after stripping only `parenthesized_expression` wrappers.

Use this when a codemod only needs simple paren normalization.

### `isUsedAsConstructor(node)`

Returns `true` when the node is effectively used as the `constructor` of a `new_expression`, even through transparent wrappers.

Example:

```ts
// true for `boundFn`
new ((0, boundFn))();
```

### `isUsedInReflectiveAccess(node, keys)`

Returns `true` when the node is used in reflective/member-introspection positions for one of the requested keys.

Handled forms include:

- member access: `node.name`
- computed access: `node["name"]`
- `in` checks: `"name" in node`

Example:

```ts
if (isUsedInReflectiveAccess(boundFnUsage, ["name", "length", "prototype", "toString"])) {
  return null;
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

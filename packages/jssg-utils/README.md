# @jssg/utils

Utilities used by the JSSG codemod engine.

## JavaScript import helpers

Import from:

```ts
import { getImport, addImport, removeImport } from "@jssg/utils/javascript/imports";
```

These helpers work on a `program` AST node (from `codemod:ast-grep` / `@codemod.com/jssg-types`) and return either **lookup info** (`getImport`) or a single **text edit** you can apply with `program.commitEdits([edit])`.

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

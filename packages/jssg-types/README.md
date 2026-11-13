# @codemod.com/jssg-types

Type declarations for [codemod](https://codemod.com): `codemod:ast-grep`, `codemod:workflow`,
`codemod:metrics`, `codemod:runtime`, `codemod:llm` and the per-language node types.

**These types now ship with the `codemod` package**, the CLI itself. This package mirrors them,
so existing imports such as `@codemod.com/jssg-types/main` keep working. For new projects, depend
on the CLI, which brings the types along:

```bash
npm install codemod
```

```json
{ "compilerOptions": { "types": ["codemod"] } }
```

and import types from `codemod/main`, `codemod/langs/tsx`, or the `codemod:*` modules directly.

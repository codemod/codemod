# JSSG Import Utilities Fallback

This file is only a compact fallback.

Public Codemod docs are the source of truth for import utility details such as:
- `getImport`
- `getAllImports`
- `addImport`
- `removeImport`
- `stringToExactRegexString`

When public docs are available, prefer them over this file.

## Default rule

If a codemod touches imports at all, check `@jssg/utils` first and use it by default whenever the needed operation is covered.

Treat helper-first import handling as the normal path, not an optional refinement. Do not hand-roll import parsing, alias discovery, import merging, import removal, or import-string reconstruction unless the helpers cannot express the required behavior.

If you bypass the helpers for an import-related change, state why.

## Core helpers

- `getImport`
  - Find whether a symbol/module is imported.
  - Return the local alias/binding used at call sites.
  - Useful for verifying that a symbol really comes from the expected module before transforming usages.

- `addImport`
  - Add a new import or merge into an existing compatible import.
  - Useful for preserving existing default/named/namespace import structure instead of rebuilding import statements manually.

- `removeImport`
  - Remove default, namespace, or named imports.
  - Useful for deleting or replacing imports without handwritten statement surgery.

- `getAllImports`
  - Find every matching import for a symbol/module instead of only the first one.
  - Useful when a file may contain multiple relevant imports that all need inspection or cleanup.

- `stringToExactRegexString`
  - Escape a string into an exact-match regex source.
  - Useful when a helper or query path needs a literal-safe regex for module names or aliases.

## Use helpers by default for

- locating whether a symbol really comes from a target module
- locating every matching import when duplicate or repeated import forms may exist
- resolving aliased import names used at runtime call sites
- preserving default/named/namespace import shape
- merging named imports into an existing statement
- handling side-effect imports
- replacing one import source with another
- matching existing ESM/CJS module style when adding imports
- avoiding text-only import rewrites that are easy to regress

## Escalation rule

Before writing custom import logic, explicitly decide whether `getImport`, `getAllImports`, `addImport`, `removeImport`, or `stringToExactRegexString` already cover the task. If they do, use them. Only drop to custom AST logic for import changes when helper behavior is genuinely insufficient.

## Common patterns

- Verify import origin before transforming usages.
  - Prefer `getImport` so aliased imports are resolved before rewriting call sites.
- Replace one import with another by composing `removeImport` and `addImport`.
  - Prefer this over handwritten import-statement surgery.
- Match existing module style when adding imports.
  - Reuse `moduleType` from an existing import when mixing ESM and CJS support matters.

## Important limitation

Import utilities reduce review churn for common import manipulation, but they do not replace semantic checks for runtime-vs-type bindings. When symbol origin matters, combine helper usage with semantic/binding-aware checks.

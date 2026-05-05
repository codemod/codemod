# JSSG Utilities Fallback

This file is only a compact fallback.

Public Codemod docs are the source of truth for utility details such as:
- `getImport`
- `getAllImports`
- `addImport`
- `removeImport`
- `stringToExactRegexString`
- `findShadowingBinding`
- `isRuntimeImportBinding`
- `getNamedChildren`
- `unwrapParenthesizedExpression`
- `isUsedAsConstructor`
- `isUsedInReflectiveAccess`

When public docs are available, prefer them over this file.

## Default rule

If a codemod touches imports, needs symbol-origin/runtime-binding checks, or needs usage-context/wrapper analysis, check `@jssg/utils` first and use it by default whenever the needed operation is covered.

Treat helper-first import, binding, and context handling as the normal path, not an optional refinement. Do not hand-roll import parsing, alias discovery, import merging, import removal, runtime-vs-type import filtering, local shadow detection, transparent-wrapper bubbling, constructor-position checks, reflective-access checks, or import-string reconstruction unless the helpers cannot express the required behavior.

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

- `findShadowingBinding`
  - Detect whether a local declaration shadows a candidate imported symbol at a usage site.
  - Useful for conservative transform gating before treating an identifier as imported.

- `isRuntimeImportBinding`
  - Detect whether a usage node resolves to a non-type-only top-level import matching a query.
  - Useful as the main gate before rewriting imported runtime symbol usages.

- `getNamedChildren`
  - Return a node's named children while skipping non-named tokens and comment nodes.
  - Useful when a codemod needs semantic child nodes without hand-filtering punctuation or trivia.

- `unwrapParenthesizedExpression`
  - Strip only `parenthesized_expression` wrappers.
  - Useful when the codemod only needs simple paren normalization.

- `isUsedAsConstructor`
  - Detect whether a node is effectively used as the constructor of a `new_expression`.
  - Useful for conservatively skipping rewrites that would change constructor behavior through wrapper expressions.

- `isUsedInReflectiveAccess`
  - Detect whether a node is used in reflective/member-introspection positions for requested keys, including `.prop`, `["prop"]`, and `"prop" in node`.
  - Useful for conservatively skipping rewrites around `name`/`length`/`prototype`/`toString` style reflection.

## Use helpers by default for

- locating whether a symbol really comes from a target module
- locating every matching import when duplicate or repeated import forms may exist
- resolving aliased import names used at runtime call sites
- detecting whether a local binding shadows an imported symbol
- gating runtime symbol rewrites conservatively before editing call sites
- stripping only parentheses before checking lightweight expression rules
- detecting effective constructor usage through wrapper expressions
- detecting reflective/introspection usage through member, subscript, and `in` forms
- preserving default/named/namespace import shape
- merging named imports into an existing statement
- handling side-effect imports
- replacing one import source with another
- matching existing ESM/CJS module style when adding imports
- avoiding text-only import rewrites that are easy to regress

## Escalation rule

Before writing custom import, binding, or usage-context logic, explicitly decide whether `getImport`, `getAllImports`, `addImport`, `removeImport`, `stringToExactRegexString`, `findShadowingBinding`, `isRuntimeImportBinding`, `getNamedChildren`, `unwrapParenthesizedExpression`, `isUsedAsConstructor`, or `isUsedInReflectiveAccess` already cover the task. If they do, use them. Only drop to custom AST logic when helper behavior is genuinely insufficient.

## Common patterns

- Verify import origin before transforming usages.
  - Prefer `getImport` so aliased imports are resolved before rewriting call sites.
- Verify runtime binding identity before rewriting symbol usages.
  - Prefer `isRuntimeImportBinding` and `findShadowingBinding` over local alias sets or raw text checks.
- Verify effective usage context before rewriting wrapped expressions.
  - Prefer `unwrapParenthesizedExpression`, `isUsedAsConstructor`, and `isUsedInReflectiveAccess` over codemod-local parent/ancestor bubbling logic.
- Read semantic child nodes from AST containers.
  - Prefer `getNamedChildren` over repeating `children().filter((child) => child.isNamed())` and similar local trivia filtering.
- Replace one import with another by composing `removeImport` and `addImport`.
  - Prefer this over handwritten import-statement surgery.
- Match existing module style when adding imports.
  - Reuse `moduleType` from an existing import when mixing ESM and CJS support matters.

## Important limitation

These utilities reduce review churn for common import, binding, and symbol-origin handling, but they do not replace semantic checks entirely. When symbol origin matters across files or through non-local indirection, combine helper usage with semantic/binding-aware checks.

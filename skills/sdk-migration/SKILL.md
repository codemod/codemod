---
name: sdk-migration
description: >-
  Plan, build, test, and publish public Codemod Registry packages for an SDK or
  library version migration. Use when the user provides a source version, target
  version, and a changelog, migration guide, or related docs link, and wants
  reusable codemods authored and published (not only applied to one repo).
allowed-tools:
  - Bash(codemod *)
  - Bash(npx codemod *)
argument-hint: "<sdk> <from-version> <to-version> <changelog-or-guide-url>"
---

# SDK Migration Codemod Authoring

codemod-compatibility: skill-package-v1
codemod-skill-version: 0.1.3

Turn an SDK/library version bump into publishable Codemod packages.

Prerequisite: the user has already run `npx codemod login` (or provided equivalent auth). Agents reuse that session — do not open a new login flow unless `whoami` fails.

## Required inputs

Collect any missing inputs before scaffolding:

| Input | Required | Notes |
| --- | --- | --- |
| SDK / package name | yes | npm/PyPI/crate name or product name |
| Source version | yes | version users migrate **from** |
| Target version | yes | version users migrate **to** |
| Changelog, migration guide, or helpful link | yes | at least one URL or pasted doc |
| Author override | no | if omitted, use the authenticated Codemod user |

## Progress checklist

```
SDK migration progress:
- [ ] 0. Auth + author resolved
- [ ] 1. Migration plan written (public by default)
- [ ] 2. Codemods built locally
- [ ] 3. Codemods tested (fix and retest until green)
- [ ] 4. Codemods published
```

## 0. Auth and author

1. Run `npx codemod whoami --detailed`.
2. If not authenticated, stop and tell the user to run `npx codemod login`, then retry.
3. Resolve package author as `Name <email>`:
   - Prefer an author the user explicitly provided.
   - Otherwise use name/email from `whoami --detailed`.
   - If email is missing, ask once; do not invent an email.
4. Note the publish scope from `whoami` (user or org). Use that scope for package names unless the user specifies another verified scope.

## 1. Plan for the migration

Read the changelog / migration guide / linked docs. Search the registry first:

```bash
npx codemod search "<sdk> <from> <to>"
npx codemod search "<sdk> migration"
```

If an existing public package already covers the exact migration, report it and ask whether to improve it or publish new packages. Do not duplicate blindly.

Write a short plan that includes:

- SDK name, source version, target version
- Source links used
- Codemod author: name and email (from auth or user-provided)
- Registry defaults: **public visibility and public access**
- Proposed package list (see packaging rules below)
- Languages / file globs
- Breaking changes in scope vs deferred / manual follow-ups
- Test strategy (fixtures + validation commands)

### Packaging rules

**Default: one registry package per SDK version bump**, named like `<sdk>-<from>-to-<to>`, with a **multi-step** `workflow.yaml`:

- One sequential `js-ast-grep` step per independent breaking change
- Separate transform scripts (keep `scripts/codemod.ts` as the first/primary step for package validation)
- Separate fixture dirs per step (e.g. `tests/<step-slug>/…`)
- Package `test` script runs `jssg test` for every step

Split into multiple packages only when:

- The user explicitly asks for separate packages, or
- Breaks target unrelated surfaces that should ship/version independently (different languages, optional product areas, or opt-in migrations)

Do **not** publish N one-step packages for the same `from → to` bump by default.

Default every new package to:

```yaml
registry:
  access: "public"
  visibility: "public"
```

Only use private access/visibility when the user explicitly asks.

Show the plan to the user, then proceed unless they revise it.

### Plan template

```markdown
# Migration plan: <sdk> <from> → <to>

- Author: <Name> <<email>>
- Registry: access=public, visibility=public
- Sources: <urls>
- Package(s):
  1. `<scope>/<sdk>-<from>-to-<to>` — multi-step: <step intents>
- Out of scope / manual: <bullets>
- Test plan: fixtures + `workflow validate` + `validate_codemod_package` + package tests
```

### Multi-step workflow shape

```yaml
version: "1"
nodes:
  - id: migrate
    name: Migrate <sdk> <from> → <to>
    type: automatic
    steps:
      - name: "<break A>"
        js-ast-grep:
          js_file: scripts/codemod.ts
          language: "typescript"
          semantic_analysis: file
          include: ["**/*.{ts,tsx}"]
          exclude: ["**/node_modules/**", "**/dist/**"]
      - name: "<break B>"
        js-ast-grep:
          js_file: scripts/<break-b>.ts
          language: "typescript"
          semantic_analysis: file
          include: ["**/*.{ts,tsx}"]
          exclude: ["**/node_modules/**", "**/dist/**"]
```

Use `semantic_analysis: workspace` when a step must resolve cross-file imports or rename across modules. Omit semantic analysis only when the transform is a pure AST shape with no shared method names and no symbol renames—and say so in the README.

## 2. Codemods built locally

Before writing transforms, read:

```bash
npx codemod ai docs codemod-creation-workflow
npx codemod ai docs jssg-gotchas
npx codemod ai docs ast-grep-gotchas
npx codemod ai docs jssg-utils
npx codemod ai docs semantic analysis
npx codemod ai docs jssg metrics
```

Scaffold **outside** product app monorepos when possible (dedicated workspace such as `~/…/<sdk>-codemods/` or `codemods/<sdk>/<target-version>/`). Do not leave migration packages under unrelated repos (e.g. app backends) unless the user asked to put them there.

For each planned package:

1. Scaffold immediately (pass only known flags; include author when resolved):

```bash
npx codemod init <path> --no-interactive --name <name> --author "Name <email>"
```

2. Set `codemod.yaml` author, description, keywords (`upgrade`, `breaking-change`, sdk name, versions), languages, and **public** registry access/visibility.
3. Replace starter transform, README, and fixtures. Wire a multi-step `workflow.yaml` when there is more than one break. Keep each step focused on one exact migration.
4. Stay AST-first with `js-ast-grep` for parser-backed languages. Use `npx codemod ai dump-ast` and `npx codemod ai node-types <lang>` when node shape is unclear.
5. **Prefer `@jssg/utils`** for JS/TS/Java: import gates (`getImport` / `listImports`), style (`getFileStyle`), renames/bindings (`importBindingOf` / `renameReferences`). Do not ship hand-rolled import pattern lists or bare identifier renames when a helper covers the case. Add `@jssg/utils` to package dependencies when used.
6. **Decide `semantic_analysis` per step** before matching generic methods (`.update`, `.get`, …), renaming types/APIs, or following aliases. Binding-aware checks beat filename-level “has import” gates alone.
7. **Emit metrics per `js-ast-grep` step**: declare `useMetricAtom` in that step’s script (statically visible name). Prefer **one metric per step**; encode variants as **cardinality** dimensions (e.g. `shape`, `kind`, `callee`). Increment on real edits. Snapshot `metrics.json` beside fixtures that produce counts.
8. Follow Performant codemods in `jssg-gotchas`: discriminating selectors/token gates, no per-file full-repo walks, timed no-op vs real run before done.
9. Align transforms, fixtures, README, `codemod.yaml`, `workflow.yaml`, and package scripts on the same migration and globs.

## 3. Codemods tested — fix and retest

For every package, do not publish until all of these pass:

```bash
npx codemod workflow validate -w <package-dir>
npx codemod ai call validate_codemod_package --input '{"package_path":"<package-dir>"}'
# then the package's default test script (npm/pnpm/yarn/bun as scaffolded)
```

Also required:

- Positive, negative, edge, and no-op fixtures **per step**
- No starter-scaffold markers left in README/transform/fixtures
- If tests or validation fail: fix the codemod, re-run the same checks, and repeat until green

Only then mark the package ready to publish.

## 4. Publish codemods

Confirm auth still works (`npx codemod whoami`), then from each package directory:

```bash
npx codemod publish
```

Report for each package:

- Registry package id / name
- Version
- Access and visibility
- Registry URL if printed by the CLI

If publish fails because of scope verification or permissions, explain the error and the docs fix (`npx codemod ai docs codemod-cli` / publishing docs). Do not silently switch to private.

## Non-negotiables

- Reuse existing login; do not invent credentials.
- Public visibility and access by default.
- Author must be the authenticated user or an author the user provided.
- Search the registry before creating duplicates.
- Default to one multi-step package per SDK version bump; do not split without cause.
- Prefer `@jssg/utils` and enable `semantic_analysis` when import origin, aliases, or generic method names matter.
- Each transform step emits metrics (`useMetricAtom` + cardinality; keep the metric count low).
- Do not stop on the first test failure — fix and retest.
- Do not commit or push git changes unless the user asked.
- Do not open a PR unless the user asked.
- Do not apply the new codemods across unrelated customer repos unless the user asked; this skill's deliverable is planned, built, tested, and published packages.

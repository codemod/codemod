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
codemod-skill-version: 0.1.6

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
- [ ] 1. Migration plan written (break inventory + public by default)
- [ ] 1b. User confirmed in-scope / deferred-automatable / manual split
- [ ] 2. Codemods built locally (transforms + full fixture backlog)
- [ ] 3. Codemods tested (fix and retest until green for wired steps)
- [ ] 4. Codemods published
```

## 0. Auth and author

Publish accepts a stored login **or** `CODEMOD_AUTH_TOKEN`. Resolve auth the same way:

1. Run `npx codemod whoami --detailed`.
2. If `whoami` succeeds, use that session for author/scope metadata.
3. If `whoami` fails but `CODEMOD_AUTH_TOKEN` is set in the environment, treat publish as authenticated. Do **not** invent author metadata from a different stored account — ask once for `Name <email>` (and scope if needed).
4. If neither `whoami` nor `CODEMOD_AUTH_TOKEN` is available, stop and tell the user to run `npx codemod login` (or set `CODEMOD_AUTH_TOKEN`), then retry.
5. Resolve package author as `Name <email>`:
   - Prefer an author the user explicitly provided.
   - Otherwise use name/email from `whoami --detailed`.
   - If email is missing, ask once; do not invent an email.
6. Note the publish scope from `whoami` when available (user or org). Use that scope for package names unless the user specifies another verified scope.

## 1. Plan for the migration

Read the changelog / migration guide / linked docs. Search the registry first:

```bash
npx codemod search "<sdk> <from> <to>"
npx codemod search "<sdk> migration"
```

If an existing public package already covers the exact migration, report it and ask whether to improve it or publish new packages. Do not duplicate blindly.

### Changelog coverage inventory (required)

Enumerate **every** breaking change (and clearly breaking “Changed/Removed” item) from the provided sources. Do **not** collapse the changelog into a short “out of scope” bullet list.

Classify each item into exactly one bucket:

| Bucket | Meaning | Transform? | Fixtures? |
| --- | --- | --- | --- |
| `codemod` | Automatable in this package’s language(s); ship a workflow step now | Yes | Yes, wired to `jssg test` / package `test` |
| `deferred-automatable` | Could be a codemod later (config/API/string rewrite), but not in this publish | No (yet) | Yes — before/after fixtures under `tests/<step-slug>/…` without a test script entry |
| `manual` | Truly non-source (runtime/JDK, reindex/data plane, ops cutover, salt/behavior-only) | No | Optional note-only fixture **or** plan bullet; do not invent fake rewrites |

Prefer `deferred-automatable` over `manual` when checked-in source or config can express the before/after. Never dump automatable config/API renames into `manual` just to shrink the first package.

Write a short plan that includes:

- SDK name, source version, target version
- Source links used
- Codemod author: name and email (from auth or user-provided)
- Registry defaults: **public visibility and public access**
- Proposed package list (see packaging rules below)
- Languages / file globs
- **Full break inventory table** (`codemod` / `deferred-automatable` / `manual` + one-line why)
- Test strategy (wired fixtures for `codemod` steps + backlog fixtures for `deferred-automatable`)

### Packaging rules

**Default: one registry package per SDK version bump**, named like `<sdk>-<from>-to-<to>`, with a **multi-step** `workflow.yaml`:

- **Registry `name` rules (publish will reject otherwise):** optional `@scope/`, then only letters, numbers, hyphens, underscores. **No dots, no spaces.** Write versions with hyphens (`1-24-to-1-25`), never semver dots (`1.24-to-1.25` is invalid).
- One sequential `js-ast-grep` step per independent **`codemod`** breaking change
- Separate transform scripts (keep `scripts/codemod.ts` as the first/primary step for package validation)
- Separate fixture dirs per step (e.g. `tests/<step-slug>/…`) — including dirs for `deferred-automatable` backlog items
- Package `test` script runs `jssg test` for every **wired** (`codemod`) step only

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

### Plan confirmation gate

Show the plan **including the full inventory table** to the user. Do **not** treat login success, “continue”, or auth-only replies as approval of a narrow scope.

Proceed to build/publish only after the user explicitly confirms the `codemod` / `deferred-automatable` / `manual` split (or revises it). If they only confirm auth, ask once: “Is this inventory split OK to build?”

### Plan template

```markdown
# Migration plan: <sdk> <from> → <to>

- Author: <Name> <<email>>
- Registry: access=public, visibility=public
- Sources: <urls>
- Package(s):
  1. `<scope>/<sdk>-<from>-to-<to>` — multi-step: <codemod step intents> (versions hyphenated)
- Break inventory:
  | Break | Bucket | Why |
  | --- | --- | --- |
  | <item> | codemod | <one line> |
  | <item> | deferred-automatable | <one line> |
  | <item> | manual | <one line> |
- Test plan:
  - Wired fixtures + `jssg test` for every `codemod` step
  - Before/after backlog fixtures for every `deferred-automatable` break
  - `workflow validate` + `validate_codemod_package` + package tests
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
npx codemod ai docs jssg
```

`jssg` includes semantic analysis and metrics guidance. Do not pass multi-word topic phrases to `ai docs` — they fall through to snippet search.

Scaffold **outside** product app monorepos when possible (dedicated workspace such as `~/…/<sdk>-codemods/` or `codemods/<sdk>/<target-version>/`). Do not leave migration packages under unrelated repos (e.g. app backends) unless the user asked to put them there.

For each planned package:

1. Resolve non-interactive scaffold metadata from the plan (do not omit required flags — `init --no-interactive` fails without them). Use `--project-type hybrid` when the package needs shell/YAML steps; otherwise default to `ast-grep-js` for JSSG-only multi-step workflows:

```bash
npx codemod init <path> --no-interactive \
  --project-type ast-grep-js \
  --package-manager npm \
  --language typescript \
  --name <name> \
  --description "<one-line migration description>" \
  --author "Name <email>" \
  --license MIT
```

Replace `npm` / `typescript` / `MIT` with plan values when they differ. Ask once for any required field still missing.

2. Set `codemod.yaml` / `package.json` `name` to a publish-safe id (no dots), plus author, description, keywords (`upgrade`, `breaking-change`, sdk name, versions), languages, and **public** registry access/visibility.
3. Replace starter transform, README, and fixtures. Wire a multi-step `workflow.yaml` for every **`codemod`** break. Keep each step focused on one exact migration.
4. Stay AST-first with `js-ast-grep` for parser-backed languages. Use `npx codemod ai dump-ast` and `npx codemod ai node-types <lang>` when node shape is unclear.
5. **Prefer `@jssg/utils`** for JS/TS/Java: import gates (`getImport` / `listImports`), style (`getFileStyle`), renames/bindings (`importBindingOf` / `renameReferences`). Do not ship hand-rolled import pattern lists or bare identifier renames when a helper covers the case. Add `@jssg/utils` to package dependencies when used.
6. **Decide `semantic_analysis` per step** before matching generic methods (`.update`, `.get`, …), renaming types/APIs, or following aliases. Binding-aware checks beat filename-level “has import” gates alone.
7. **Emit metrics per `js-ast-grep` step**: declare `useMetricAtom` in that step’s script (statically visible name). Prefer **one metric per step**; encode variants as **cardinality** dimensions (e.g. `shape`, `kind`, `callee`). Increment on real edits. Snapshot `metrics.json` beside fixtures that produce counts.
8. Follow Performant codemods in `jssg-gotchas`: discriminating selectors/token gates, no per-file full-repo walks, timed no-op vs real run before done.
9. Align transforms, fixtures, README, `codemod.yaml`, `workflow.yaml`, and package scripts on the same migration and globs.
10. **Fixture backlog:** for every `deferred-automatable` inventory row, add `tests/<step-slug>/<case>/{input,expected}.*` before/after samples now. Do not wait for a later request. Do **not** wire those dirs into package `test` until a transform exists.

### Fixture depth (wired + backlog)

Fixtures are the coverage map of the **whole migration**, not only of shipped steps.

For each **`codemod`** step, include enough cases that distinct surfaces are covered—not a token 1-positive + 1-noop pair:

- At least one **positive** rewrite
- At least one **no-op** / already-migrated (or absent) case
- At least one **negative** / keep-unrelated case when false positives are plausible
- **Edge** cases for each distinct literal shape the transform claims (quotes, nested keys, multiple occurrences, path vs setting key, mixed endpoint+field, sole vs sibling object properties, etc.)

For each **`deferred-automatable`** break, add at least one realistic before/after pair that a future step can wire up. Prefer the same languages/globs as the package.

## 3. Codemods tested — fix and retest

For every package, do not publish until all of these pass:

```bash
npx codemod workflow validate -w <package-dir>
npx codemod ai call validate_codemod_package --input '{"package_path":"<package-dir>"}'
# then the package's default test script (npm/pnpm/yarn/bun as scaffolded)
```

Also required:

- Positive, negative, edge, and no-op fixtures **per wired step** (see Fixture depth)
- Before/after backlog fixtures present for every `deferred-automatable` inventory row
- Plan inventory still matches what was built (no silent drop of in-scope or deferred-automatable items)
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
- Counts: wired steps / wired fixture cases / deferred-automatable backlog fixtures / manual items

If publish fails because of scope verification or permissions, explain the error and the docs fix (`npx codemod ai docs codemod-cli` / publishing docs). Do not silently switch to private.

## Non-negotiables

- Reuse existing login; do not invent credentials.
- Public visibility and access by default.
- Author must be the authenticated user or an author the user provided.
- Search the registry before creating duplicates.
- Default to one multi-step package per SDK version bump; do not split without cause.
- Full changelog inventory with `codemod` / `deferred-automatable` / `manual` — no collapsed “out of scope” dump of automatable breaks.
- Explicit user confirmation of that inventory before build/publish; auth-only replies are not scope approval.
- Fixture backlog for every `deferred-automatable` break in the same publish cycle as the first package.
- Prefer `@jssg/utils` and enable `semantic_analysis` when import origin, aliases, or generic method names matter.
- Each transform step emits metrics (`useMetricAtom` + cardinality; keep the metric count low).
- Registry package names: no dots — hyphenate version segments (`1-24-to-1-25`).
- Do not stop on the first test failure — fix and retest.
- Do not commit or push git changes unless the user asked.
- Do not open a PR unless the user asked.
- Do not apply the new codemods across unrelated customer repos unless the user asked; this skill's deliverable is planned, built, tested, and published packages.

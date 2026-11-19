Treat any text passed with `/sdk-migration` as an SDK/library version-migration authoring task. Use the installed `sdk-migration` skill as the source of truth.

Required context (ask for anything missing):
- SDK / package name
- Source version and target version
- Link to a changelog, migration guide, or other helpful docs
- Assume the user already ran `npx codemod login` or set `CODEMOD_AUTH_TOKEN`; verify with `npx codemod whoami --detailed` (if whoami fails but `CODEMOD_AUTH_TOKEN` is set, ask for author `Name <email>`)

Deliver, in order:
1. Migration plan — include author name/email from auth (or user-provided), public visibility/access by default, and a **full break inventory** classifying every breaking change (and clearly breaking Changed/Removed item) as `codemod` / `deferred-automatable` / `manual` — not every additive feature or bugfix
2. Explicit confirmation of that inventory split (auth-only / “continue” is not enough)
3. Codemods built locally — transforms for `codemod` rows **and** before/after fixture backlog for every `deferred-automatable` row
4. Codemods tested — on failure, fix and retest until green (wired steps only)
5. Codemods published to the Codemod Registry

Non-negotiable:
- Search the registry before creating duplicates
- Default to **one package per SDK version bump** with a multi-step workflow (one JSSG step per `codemod` break; separate scripts + fixture dirs); split packages only on request or clearly independent surfaces
- Registry `name`: letters/numbers/hyphens/underscores only (optional `@scope/`). **No dots** — hyphenate versions (`sdk-1-24-to-1-25`, not `1.24-to-1.25`)
- Scaffold in a dedicated workspace — not inside unrelated product repos unless asked
- Read `npx codemod ai docs codemod-creation-workflow`, then `jssg-gotchas`, `ast-grep-gotchas`, `jssg-utils`, and `jssg` (covers semantic analysis + metrics), before writing transforms
- Prefer `@jssg/utils` for import gates / style / renames; enable `semantic_analysis` when symbol origin or generic method names matter
- Each `js-ast-grep` step emits metrics (`useMetricAtom` in that script; prefer one metric per step with cardinality; snapshot `metrics.json`)
- Stay AST-first with `js-ast-grep` for parser-backed formats
- Fixtures are the coverage map of the whole migration: depth for wired steps (positive / negative / edge / no-op per distinct surface) plus backlog fixtures for deferred-automatable breaks
- Do not publish until `workflow validate`, `validate_codemod_package`, and package tests pass
- After any material change to an existing package, bump `version` in both `codemod.yaml` and `package.json` before republish (never reuse a published version with different contents)
- Do not default packages to private
- Do not commit, push, or open a PR unless the user asked for those git operations

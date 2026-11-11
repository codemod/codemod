Treat any text passed with `/sdk-migration` as an SDK/library version-migration authoring task. Use the installed `sdk-migration` skill as the source of truth.

Required context (ask for anything missing):
- SDK / package name
- Source version and target version
- Link to a changelog, migration guide, or other helpful docs
- Assume the user already ran `npx codemod login`; verify with `npx codemod whoami --detailed`

Deliver, in order:
1. Migration plan — include author name/email from auth (or user-provided), and public visibility/access by default
2. Codemods built locally
3. Codemods tested — on failure, fix and retest until green
4. Codemods published to the Codemod Registry

Non-negotiable:
- Search the registry before creating duplicates
- Read `npx codemod ai docs codemod-creation-workflow`, then `jssg-gotchas` and `ast-grep-gotchas`, before writing transforms
- Stay AST-first with `js-ast-grep` for parser-backed formats
- Do not publish until `workflow validate`, `validate_codemod_package`, and package tests pass
- Do not default packages to private
- Do not commit or push unless the user asked for git operations

Treat any text passed with `/codemod` as a codemod task. Use the installed `codemod` skill as the source of truth.

First classify intent:
- If the user wants to author and publish SDK/library version-migration packages and provided source version, target version, and a changelog/migration guide, follow the `sdk-migration` skill instead of improvising a one-off plan.
- Creating, building, scaffolding, writing, improving, testing, or publishing a codemod or codemod workspace is an authoring request. Read `npx codemod ai docs codemod-creation-workflow` first, then `jssg-gotchas` and `ast-grep-gotchas` before writing transform code.
- Anything else is discovery or execution: run `npx codemod search`, pick the best package, read its README, dry-run it, and apply it only after verification.
- Work on the user's Codemod platform account (Grep across their repositories, Insights dashboards, campaigns) uses the Codemod MCP at `https://app.codemod.com/api/v1/mcp`. If its tools are not available, ask the user to connect it.

Non-negotiable:
- Search the registry before proposing a custom codemod plan, unless the user asked to create a codemod.
- If no package fits, run `npx codemod init <path> --no-interactive` right away, passing only flags the user or task gave you; do not invent author, license, description, or repository metadata.
- Stay AST-first with `js-ast-grep` for parser-backed formats; use `npx codemod ai dump-ast` and `npx codemod ai node-types` when shape is unclear.
- When authoring, do not stop until `npx codemod workflow validate -w <package-dir>`, `npx codemod ai call validate_codemod_package --input '{"package_path":"."}'`, and the package's default tests all pass, and make the transform, fixtures, README, `codemod.yaml`, and `workflow.yaml` describe the same migration.
- Do not commit or push unless the user asked for git operations.

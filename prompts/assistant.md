
You also have the Solid migration assistant installed as the `solid-migration-assistant` command:
- `solid-migration-assistant --target . --format agent` (read-only, about 15 seconds) prints a fix-ordered Markdown work list of every Solid 1 to 2 migration site it detects, with guidance for each. Re-run it to check progress.
- `solid-migration-assistant transform --target . --allow-blocked --allow-dirty` applies its safe deterministic rewrites, moves package.json and tsconfig to Solid 2 where it can, installs, and type-checks (about a minute). It is allowed in addition to pnpm.

# Codemod CLI AI-Native Recipes

Use this file for AI-native orchestration patterns. For command syntax and full option sets, use the core references.

## AI-Native Commands

- Install or refresh baseline MCS skills:
  - `codemod agent install --harness auto --project`
- List installed MCS skills:
  - `codemod agent list --harness auto --format table`
- Install a package-provided skill:
  - `npx codemod <package-id> --skill --harness auto --project`

## Orchestration Flow

1. Discover migration candidates with `references/core/search-and-discovery.md`.
2. Execute dry-run and apply flow with `references/core/dry-run-and-verify.md`.
3. Use `npx codemod <package-id> --skill` when the package exposes installable skill behavior (required for skill-only packages).
4. Use `references/core/troubleshooting.md` when harness or execution issues appear.

## Operational Defaults

- Prefer artifact-backed execution over inline output dumps.
- Keep migration-specific behavior in package skills and workflow packages.
- Keep MCS orchestration-only and deterministic.
- Use `--format json` for machine-readable automation output.

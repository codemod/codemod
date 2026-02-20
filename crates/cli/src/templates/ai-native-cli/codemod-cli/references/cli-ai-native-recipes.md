# Codemod CLI AI-Native Recipes

Use this file for AI-native command recipes. For all references, start at `references/index.md`.

## AI-Native Entry Commands

- Install MCS skill pack (also bootstraps Codemod MCP config): `codemod agent install-skills --harness auto --project`
- Verify installed skills: `codemod agent verify-skills --harness auto --format json`
- Run orchestration preflight: `codemod agent run "<intent>" --harness auto --format json`
- Install a specific TCS: `codemod tcs install <tcs-id> --harness auto --project`

## Fallback Recipe (Metadata Gaps)

If `agent run` returns `insufficient_metadata` or `no_candidates`:
- Discover candidates: `codemod search "<migration>" --format json`
- Validate candidate safely: `codemod run <package-name> --dry-run --target <repo-path>`
- Apply after review: `codemod run <package-name> --target <repo-path>`

## Core CLI Reference Map

- Search and discovery:
  - `references/cli-core-search-and-discovery.md`
- Scaffold and apply:
  - `references/cli-core-scaffold-and-run.md`
- Dry run and verification:
  - `references/cli-core-dry-run-and-verify.md`
- Troubleshooting and ops:
  - `references/cli-core-troubleshooting.md`

## Operational Defaults

- Prefer artifact-backed execution over inline output dumps.
- Keep migration-specific behavior in TCS packages.
- Keep MCS orchestration-only and deterministic.
- Use `--format json` for machine-readable automation output.

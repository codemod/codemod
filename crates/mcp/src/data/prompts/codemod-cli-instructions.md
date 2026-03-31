# Fallback Codemod CLI Guidance

This file is a last-resort fallback used only when the public Codemod docs cannot be fetched at runtime.

The public docs are the source of truth for:
- CLI usage
- package structure
- workflow authoring
- sharding
- Campaign/cloud git automation semantics

## Fallback rules

- Prefer the public docs and current CLI help over this file whenever they are available.
- If exact syntax is uncertain, use help instead of guessing:
  - `codemod --help`
  - `codemod workflow --help`
  - `codemod jssg --help`
  - `codemod init --help`
- For migration, upgrade, update, or deprecation-rollout requests, run registry discovery before proposing a custom codemod.
- Validate local workflows before running them.
- Dry-run before apply.
- Prefer `npx codemod@latest ...` or a verified local Codemod binary when the plain `codemod` command resolves to an unexpected wrapper.

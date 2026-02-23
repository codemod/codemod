---
name: "{name}"
description: "{description}"
allowed-tools:
  - Bash(codemod *)
---

# {name}

codemod-compatibility: skill-package-v1
codemod-skill-version: 0.1.0

Use `references/index.md` as the primary instruction index for this package.

## Execution Contract

1. Load package-specific guidance from `references/index.md`.
2. Apply the package strategy to the current repository context.
3. Report what changed, what was skipped, and any manual follow-ups.

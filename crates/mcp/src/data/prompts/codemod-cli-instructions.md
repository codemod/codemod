# Codemod CLI Fallback

This file is only a compact fallback.

Public Codemod docs are the source of truth for:
- CLI usage and flags
- package structure
- workflow syntax
- run/validate/test commands

When public docs are available, prefer them over this file.

Agent-only reminders:
- Prefer the current CLI help and public docs over guessed commands.
- Quote multi-word registry search queries.
- For codemod authoring, do not continue open-ended planning after a registry miss; scaffold a package first with direct `codemod init`.
- In headless/non-interactive scaffolding, use `codemod init <path> --no-interactive` and pass only values the user or task actually specified. Do not invent `--author`, `--license`, `--description`, or `--git-repository-url`; the simplified CLI defaults package metadata and publish can infer a missing author from the authenticated user.
- Preserve the scaffold-selected package manager in `package.json` scripts and package-local README commands. Use the package's own runner (`yarn`, `pnpm`, `bun`, or `npm`) consistently instead of rewriting it to another one.
- For reusable authored codemods, do not default registry access/visibility to private unless the user explicitly asked for a private package.

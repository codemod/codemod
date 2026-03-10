---
name: codemod
description: Plan and execute code migrations, and create codemod packages or monorepos with Codemod CLI using safe, repeatable workflows.
allowed-tools:
  - Bash(codemod *)
argument-hint: "<migration-intent>"
---

# Codemod Migration Assistant

codemod-compatibility: mcs-v1
codemod-skill-version: 1.0.0

Use this skill to orchestrate migration execution.

Recommended runtime flow:
1. Discover candidates with `codemod search`.
2. Run workflow-capable packages with `codemod run --dry-run` before apply.
3. Run `codemod <package-id>` and accept the install prompt when a package exposes installable skill behavior (required for skill-only packages).
4. Enforce verification with tests and dry-run summaries before apply.

For codemod creation:
- Start with `references/core/create-codemods.md`.
- Load `references/core/maintainer-monorepo.md` when the user is building or maintaining a codemod monorepo.

For command-level guidance:
- Start with `references/index.md`.
- Load only the specific reference file needed for the current task.

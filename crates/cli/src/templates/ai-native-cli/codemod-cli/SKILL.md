---
name: codemod
description: Plan and execute code migrations with Codemod CLI using safe, repeatable workflows.
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
3. Install package-specific skills with `npx codemod <package-id> --skill` for skill-only packages or when package-specific execution guidance is needed.
4. Enforce verification with tests and dry-run summaries before apply.

For command-level guidance:
- Start with `references/index.md`.
- Load only the specific reference file needed for the current task.

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
- Load `references/core/maintainer-monorepo.md` when the user is building or maintaining a codemod monorepo, or when the migration spans multiple documented version hops.
- For non-trivial codemod creation, decompose the work into focused sub-agents for research, codebase analysis, implementation, and testing instead of keeping the whole task in one context.
- Use AST-based edits for JS/TS code transforms. If a code change cannot be implemented safely with AST tooling, leave it manual instead of falling back to raw source-string or regex rewrites.
- If a hop is manual-only, justify that decision from research and encode the rationale in the package docs.
- Tests and README command examples must cover the real user-requested scope and current CLI surface, not just one happy-path fixture or guessed commands.

For command-level guidance:
- Start with `references/index.md`.
- Load only the specific reference file needed for the current task.

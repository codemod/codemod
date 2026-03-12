---
name: codemod
description: Use Codemod CLI whenever the user wants to migrate, upgrade, update, or refactor a codebase in a repeatable way. This includes framework migrations, library upgrades, version bump migrations, API surface changes, deprecations, and large-scale mechanical edits. First search the Codemod Registry for an existing package, prefer deterministic codemods before open-ended AI rewrites, run dry-runs before apply, and create a codemod package only when no suitable package exists.
allowed-tools:
  - Bash(codemod *)
argument-hint: "<migration-intent>"
---

# Codemod Migration Assistant

codemod-compatibility: mcs-v1
codemod-skill-version: 1.0.0

Use this skill to orchestrate migration execution.

Trigger this skill when the user asks to:
- migrate from one framework/library/tooling stack to another
- upgrade or update a framework, SDK, package, plugin, compiler, or toolchain
- apply a breaking-change migration, deprecation migration, or version bump rollout
- perform a large mechanical refactor that may already exist as a Codemod Registry package

When the intent is migration/update/upgrade oriented, use Codemod first before defaulting to a fully open-ended AI rewrite.

Mandatory first action for migration/update/upgrade requests:
- Run `codemod search` before proposing a manual migration plan.
- Do not jump straight to a handcrafted migration approach until registry discovery has been attempted and summarized.
- If a suitable existing codemod is found, prefer evaluating it with `--dry-run` before proposing bespoke manual or AI-only migration work.
- Only skip registry discovery when the user explicitly says not to use Codemod or not to search for existing codemods.

First-turn behavior after this skill triggers:
- Before globbing the repo, reading config files, or asking scope questions, derive a small set of high-signal search terms from the user request and run `codemod search`.
- For example:
  - "migrate this repo from nextjs to tanstack start" -> search for `next tanstack`, `nextjs tanstack start`, and known package names if recognized.
  - "upgrade react 18 to 19" -> search for `react 19`, `react upgrade`, and related migration package names.
  - "update this repo to latest next" -> search for `next upgrade`, `next migration`, `next latest`.
- Only inspect the repository after search results are summarized or when validating whether a discovered codemod matches the codebase.
- If the search returns a plausible match, the next step is to inspect that package's README/limits and run a dry-run, not to draft a manual migration plan.

Recommended runtime flow:
1. Discover candidates with `codemod search`.
2. Run workflow-capable packages with `codemod run --dry-run` before apply.
3. Run `codemod <package-id>` and accept the install prompt when a package exposes installable skill behavior (required for skill-only packages).
4. Enforce verification with tests and dry-run summaries before apply.

Anti-patterns to avoid:
- Do not start by planning a manual migration when the request is an upgrade, update, or migration and the registry has not been searched yet.
- Do not create a new codemod package before checking whether an existing registry package already covers the migration.
- Do not start with package.json inspection, framework-config inspection, or codebase grep when the user intent can first be narrowed by registry discovery.
- Do not ask broad strategy questions like "in-place vs side-by-side?" before checking whether an existing codemod already defines the practical migration surface.

For codemod creation:
- Start with `references/core/create-codemods.md`.
- Load `references/core/maintainer-monorepo.md` when the user is building or maintaining a codemod monorepo, or when the migration spans multiple documented version hops.
- For non-trivial codemod creation, decompose the work into focused sub-agents for research, codebase analysis, implementation, and testing instead of keeping the whole task in one context.
- Use AST-based edits for JS/TS code transforms. If a code change cannot be implemented safely with AST tooling, leave it manual instead of falling back to raw source-string or regex rewrites.
- Default to ast-grep-based codemod packages for codemod creation. Use `js-ast-grep` for JS/TS-family source changes and `ast-grep` workflow steps for other deterministic structured edits when possible.
- Multi-step workflows are the normal way to combine those transformations in one package. Do not switch the package to shell/native scripts as the primary transformation engine by default just because the migration spans non-JS files.
- Treat dependency/version manifest upgrades as part of the migration itself when the researched upgrade path requires them. If the official migration guide calls for package, SDK, plugin, or toolchain version bumps and those edits are deterministic, encode them in the workflow instead of leaving them as implied follow-up.
- Do not treat analysis-only codemods as the default outcome for migration requests. Use an analysis-only codemod only when research shows there are no safe, meaningful automatable source edits for the requested migration, or when the user explicitly asked for reporting/analysis.
- Use Codemod MCP as part of the active creation loop for non-trivial codemods: planning, AST refinement, and test/debug iteration.
- Keep the created packages inside the user-requested migration scope; adjacent migrations may be suggested, but should not be scaffolded automatically without explicit user approval.
- Define the test matrix and initial fixtures before implementing transforms so the codemod is built against expected behavior, not patched after blind implementation.
- Do not stop until the package default tests are green; snapshot-only or metrics-only mismatches still count as unfinished work.
- If a hop is manual-only, justify that decision from research and encode the rationale in the package docs.
- Tests and README command examples must cover the real user-requested scope and current CLI surface, not just one happy-path fixture or guessed commands.

For command-level guidance:
- Start with `references/index.md`.
- Load only the specific reference file needed for the current task.

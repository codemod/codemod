---
name: codemod
description: Use Codemod CLI whenever the user wants to migrate, upgrade, update, or refactor a codebase in a repeatable way. This includes framework migrations, library upgrades, version bump migrations, API surface changes, deprecations, and large-scale mechanical edits. First search the Codemod Registry for an existing package, prefer deterministic codemods before open-ended AI rewrites, run dry-runs before apply, and create a codemod package only when no suitable package exists.
allowed-tools:
  - Bash(codemod *)
  - Bash(npx codemod *)
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

## MCP invocation guarantees

When the user:
- **Creates a codemod or does a large refactor** — Call `get_jssg_instructions`, `get_codemod_cli_instructions`, and `get_codemod_creation_workflow` from Codemod MCP before proceeding.
- **Maintains a codemod monorepo** — Call `get_codemod_maintainer_monorepo` from Codemod MCP.
- **Runs or discovers codemods** — Call `get_codemod_cli_instructions` for command syntax.
- **Hits errors or unexpected behavior** — Call `get_codemod_troubleshooting` from Codemod MCP.
- **Needs import manipulation helpers** — Call `get_jssg_utils_instructions` from Codemod MCP.
- **Needs to split a large migration into multiple PRs** — Read the `sharding-instructions` resource from Codemod MCP.

## Authoring defaults

- Use these defaults even before MCP responds.
- Default to `ast-grep-js` when the requested codemod is mainly JS/TS-family source edits.
- Choose `hybrid` only when shell/native orchestration or multiple deterministic transformation surfaces are a core part of the package.
- Do not choose `hybrid` merely because the codemod may need to create a helper file, test fixture, preview route, or README update.
- For non-interactive scaffolding, rely on the public CLI docs or `codemod init --help` for the current required flags instead of guessing them.
- Quote multi-word registry search queries.
- Prefer `npx codemod@latest ...` or a verified local Codemod binary when the plain `codemod` command behaves unexpectedly.

## Runtime flow (default)

1. Discover candidates with `codemod search`.
2. Read the selected package's README/docs and perform any documented prerequisites or setup steps.
3. Run workflow-capable packages with `codemod run --dry-run` before apply.
4. Run `codemod <package-id>` and accept the install prompt when a package exposes installable skill behavior (required for skill-only packages).
5. Enforce verification with tests and dry-run summaries before apply.

## Mandatory first action for migration/update/upgrade requests

- Run `codemod search` before proposing a manual migration plan.
- Do not jump straight to a handcrafted migration approach until registry discovery has been attempted and summarized.
- If a suitable existing codemod is found, prefer evaluating it with `--dry-run` before proposing bespoke manual or AI-only migration work.
- Only skip registry discovery when the user explicitly says not to use Codemod or not to search for existing codemods.

## First-turn behavior

- Before globbing the repo, reading config files, or asking scope questions, derive a small set of high-signal search terms from the user request and run `codemod search`.
- Only inspect the repository after search results are summarized or when validating whether a discovered codemod matches the codebase.
- If the search returns a plausible match, the next step is to inspect that package's README/limits and run a dry-run, not to draft a manual migration plan.

## Anti-patterns to avoid

- Do not start by planning a manual migration when the request is an upgrade, update, or migration and the registry has not been searched yet.
- Do not create a new codemod package before checking whether an existing registry package already covers the migration.
- Do not start with package.json inspection, framework-config inspection, or codebase grep when the user intent can first be narrowed by registry discovery.
- Do not ask broad strategy questions like "in-place vs side-by-side?" before checking whether an existing codemod already defines the practical migration surface.
- Do not run a discovered package blindly without first reading its README/docs for prerequisites, config, and known limits.

## User preferences (override defaults here if needed)

<!-- Empty by default. User adds e.g.: -->
<!-- - Always use --no-interactive -->
<!-- - Prefer pnpm for codemod init -->
<!-- - Use --strictness ast for jssg test -->

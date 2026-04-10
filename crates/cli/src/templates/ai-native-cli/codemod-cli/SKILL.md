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

- If the expected Codemod MCP tools are not actually available in the callable tool list for this session, stop codemod authoring immediately. Tell the user to reload/restart Codex and fix the Codemod MCP setup first. Do not continue codemod creation without MCP.

When the user:
- **Creates a codemod or does a large refactor** — Read `codemod-creation-workflow-instructions` first. Before writing source-transform code, read `jssg-gotchas` and `ast-grep-gotchas`. Read `codemod-cli-instructions` only when you need exact command syntax. Read `jssg-instructions` once a package exists and you are implementing the transform.
- **Needs to know whether a codemod package is still a starter scaffold or incomplete** — Call `validate_codemod_package` from Codemod MCP before stopping.
- **Needs Node/LLRT APIs, capability-gated modules, or non-trivial multi-file JSSG work** — Read `jssg-runtime-capabilities-instructions` from Codemod MCP.
- **Maintains a codemod monorepo** — Read `codemod-maintainer-monorepo-instructions` from Codemod MCP.
- **Runs or discovers codemods** — Read `codemod-cli-instructions` for command syntax.
- **Hits errors or unexpected behavior** — Read `codemod-troubleshooting-instructions` from Codemod MCP.
- **Needs import manipulation helpers** — Read `jssg-utils-instructions` from Codemod MCP.
- **Needs to split a large migration into multiple PRs** — Read the `sharding-instructions` resource from Codemod MCP.

## Authoring defaults

- Treat the public Codemod docs served through MCP as the source of truth for CLI, workflow, and JSSG semantics.
- Keep source transforms AST-first. Do not use regex or raw source-text rewriting as the primary implementation strategy.
- Use `dump_ast` before broadening heuristics.
- If symbol origin matters, use semantic analysis and binding-aware checks.
- Keep one granular transform or one exact `from -> to` migration as a single package unless the request is clearly open-ended or multi-hop.
- Inspect 1-3 representative repo files after or alongside registry discovery before you finalize the transform shape.
- If registry search yields no exact package, run `codemod init` immediately instead of continuing broad research without a package. In headless/non-interactive flows, use the simplified `codemod init <path> --no-interactive` interface and pass only user- or task-provided flags; do not invent `--author`, `--license`, `--description`, or `--git-repository-url`.
- After the package exists, replace the starter transform, README, and starter fixtures before doing optional work.
- Define positive, negative, and edge fixtures before deep implementation work.
- Before stopping, inspect the whole package surface and update every affected file together: `README.md`, `codemod.yaml`, `workflow.yaml`, `package.json` scripts, tests/fixtures, and any renamed paths, ids, or references. Do not churn version numbers by default, but do not leave stale package metadata behind after a rename or material package-surface change.
- Preserve the scaffold-selected package manager in `package.json` scripts and package-local README/development commands. Infer it from the scaffold choice, lockfile, or existing package metadata; do not rewrite `yarn`/`pnpm`/`bun` packages to `npx`/`npm` unless the user explicitly asked.
- Do not stop while `validate_codemod_package` still reports starter scaffold markers, missing package surface updates, missing real test cases, or failing default tests.
- For reusable authored codemods, do not default registry access/visibility to private unless the user explicitly asked for a private package.
- Leave missing package author metadata to the CLI defaults/publish-time auth fallback unless the user supplied an explicit author.
- Do not create commits or push branches for codemod authoring/evaluation unless the user explicitly asked for git operations.

## Runtime flow (default)

1. Discover candidates with `codemod search`.
2. Read the selected package's README/docs and perform any documented prerequisites or setup steps.
3. Run workflow-capable packages with `codemod run --dry-run` before apply.
4. Run `codemod <package-id>` and accept the install prompt when a package exposes installable skill behavior.
5. Enforce verification with tests and dry-run summaries before apply.

## Mandatory first action for migration/update/upgrade requests

- Run `codemod search` before proposing a manual migration plan.
- Do not jump straight to a handcrafted migration approach until registry discovery has been attempted and summarized.
- If a suitable existing codemod is found, prefer evaluating it with `--dry-run` before proposing bespoke manual or AI-only migration work.
- Only skip registry discovery when the user explicitly says not to use Codemod or not to search for existing codemods.

## First-turn behavior

- Before broad repo inspection or planning, derive a small set of high-signal search terms from the user request and run `codemod search`.
- For codemod authoring, inspect only a small representative slice of the repo after or alongside registry discovery, then scaffold and iterate.
- If the search returns a plausible existing package, the next step is to inspect that package's README/limits and run a dry-run, not to draft a manual migration plan.

## Anti-patterns to avoid

- Do not start by planning a manual migration when the request is an upgrade, update, or migration and the registry has not been searched yet.
- Do not create a new codemod package before checking whether an existing registry package already covers the migration.
- Do not keep reading broad guidance after a registry miss without scaffolding a package.
- Do not run a discovered package blindly without first reading its README/docs for prerequisites, config, and known limits.
- Do not introduce a shell step just to reach or mutate another related file path when JSSG can handle the hop.
- Do not continue codemod authoring when Codemod MCP is missing from the callable tool list.

## User preferences (override defaults here if needed)

<!-- Empty by default. User adds e.g.: -->
<!-- - Always use --no-interactive -->
<!-- - Prefer pnpm for codemod init -->
<!-- - Use --strictness ast for jssg test -->

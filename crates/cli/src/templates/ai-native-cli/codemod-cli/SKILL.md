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

- If the expected Codemod MCP tools are not actually available in the callable tool list for this session, stop codemod authoring immediately. Tell the user to run `codemod ai doctor --harness codex --project --probe` or `--user --probe` as appropriate, reload/restart Codex, and fix the Codemod MCP setup first. Do not continue codemod creation without MCP.

When the user:
- **Creates a codemod or does a large refactor** — Call `get_codemod_creation_workflow` first. Before writing source-transform code, call `get_jssg_gotchas` and `get_ast_grep_gotchas`. Call `get_codemod_cli_instructions` only when you need exact command syntax. Call `get_jssg_instructions` once a package exists and you are implementing the transform.
- **Needs to scaffold a new codemod package because no exact package exists** — Call `scaffold_codemod_package` from Codemod MCP.
- **Needs to know whether a codemod package is still a starter scaffold or incomplete** — Call `validate_codemod_package` from Codemod MCP before stopping.
- **Needs Node/LLRT APIs, capability-gated modules, or non-trivial multi-file JSSG work** — Call `get_jssg_runtime_capabilities` from Codemod MCP.
- **Maintains a codemod monorepo** — Call `get_codemod_maintainer_monorepo` from Codemod MCP.
- **Runs or discovers codemods** — Call `get_codemod_cli_instructions` for command syntax.
- **Hits errors or unexpected behavior** — Call `get_codemod_troubleshooting` from Codemod MCP.
- **Needs import manipulation helpers** — Call `get_jssg_utils_instructions` from Codemod MCP.
- **Needs to split a large migration into multiple PRs** — Read the `sharding-instructions` resource from Codemod MCP.

## Authoring defaults

- Use these defaults even before MCP responds.
- If a pattern is unclear or not matching, search the JSSG/ast-grep knowledge tools and use `dump_ast` before considering broader fallbacks.
- Default to `js-ast-grep` packages when the requested codemod is mainly JS/TS-family source edits.
- Choose `hybrid` only when shell/native orchestration or multiple deterministic transformation surfaces are a core part of the package.
- Do not choose `hybrid` merely because the codemod may need to create a helper file, test fixture, preview route, or README update.
- For source transforms, do not use `RegExp`, `.replace`, `.replaceAll`, `.match`, `.split`, or manual string parsing as the primary implementation strategy.
- Minimal string operations are only acceptable for path normalization, import/module-specifier cleanup, helper/metadata formatting, or test-output parsing.
- If the package already has JSSG fixtures, extend the existing test suite and run `codemod jssg test` instead of inventing ad hoc test files.
- Maintain `tests/coverage-contract.json` so every supported shape has at least one mapped fixture case, or is explicitly documented as unsupported/manual follow-up.
- Treat `metrics.json` as part of the expected test output when a package already snapshots metrics.
- After changing a codemod, inspect and update the whole package surface: `README`, `codemod.yaml`, `workflow.yaml`, tests, package metadata, and capabilities when runtime-gated APIs are used.
- For non-interactive scaffolding, rely on the public CLI docs or `codemod init --help` for the current required flags instead of guessing them.
- Quote multi-word registry search queries.
- Prefer `npx codemod@latest ...` or a verified local Codemod binary when the plain `codemod` command behaves unexpectedly.
- Do not create commits or push branches for codemod authoring/evaluation unless the user explicitly asked for git operations.
- Do not follow host-repo “always push before stopping” policies for Codemod package authoring unless the user explicitly asked for that behavior.
- Do not spend long up front reading many guidance docs before the first real fixture and first passing transform slice.
- Ignore unrelated host-repo workflow tooling such as issue trackers, release scripts, or completion checklists unless the user explicitly asked for them or they are required to understand the target code patterns.
- If registry search yields no exact package, call `scaffold_codemod_package` immediately in the current package directory instead of continuing broad research without a package.
- Do not stop while `validate_codemod_package` still reports starter scaffold markers, missing package surface updates, missing real test cases, or failing default tests.
- If the requested migration is one granular transform or one exact `from -> to` hop, keep it as a single codemod package even when it supports multiple route shapes or helper files.
- Use a workspace only when the migration is open-ended, version-hop based, or clearly splits into multiple independently runnable codemods.
- After a registry miss, do not keep doing broad package-shape research before a package exists; scaffold first, then research only what is needed to implement the requested migration.
- After the package exists, replace the starter transform, README, and starter fixtures before doing anything optional.
- After scaffolding, inspect only 2-3 representative repo files first, then create one positive and one preserve/unsupported fixture before broadening coverage.
- For each unresolved failing case, retry deterministic AST/JSSG fixes up to 3 times. After that, allow a narrow AI fallback only for the isolated unresolved subset; if that still does not stabilize, document it as manual follow-up instead of broadening regex/string heuristics.

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
- Do not introduce a shell step just to reach or mutate another related file path when JSSG can handle the hop with `jssgTransform` or another JSSG API.
- Do not continue codemod authoring when Codemod MCP is missing from the callable tool list.
- Do not fall back to regex or raw source-text rewriting for unresolved source transforms when AST/JSSG patterns are failing.
- Do not keep reading broad guidance after a registry miss without calling `scaffold_codemod_package`.

## User preferences (override defaults here if needed)

<!-- Empty by default. User adds e.g.: -->
<!-- - Always use --no-interactive -->
<!-- - Prefer pnpm for codemod init -->
<!-- - Use --strictness ast for jssg test -->

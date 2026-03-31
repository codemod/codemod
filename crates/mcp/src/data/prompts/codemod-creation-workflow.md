# Supplemental Codemod Creation Guidance

This file is a supplemental local guide.

Public docs should remain the source of truth for:
- CLI usage
- package structure
- workflow authoring
- JSSG usage
- testing patterns

Use this local guide only for extra agent workflow policy that is not yet modeled directly in the public docs.

## Source hierarchy

- Treat the public Codemod docs as the primary source of truth when they exist.
- Use this file for agent workflow policy, not for replacing the docs.
- If public docs and this file disagree on public CLI/workflow semantics, prefer the public docs.

## Default process

1. Search for an existing codemod before creating a new one.
2. Inspect the target codebase for representative real examples.
3. Research the migration path before choosing package shape.
4. Define the test matrix before implementing transforms.
5. Implement against those tests.
6. Keep iterating until the package tests pass.

## Non-negotiable rules

- Use AST-based transforms for JS/TS code changes.
- Prefer `js-ast-grep` for JS/TS-family edits and workflow steps for other deterministic structured files.
- Use multi-step workflows when the migration spans multiple safe transformation surfaces.
- Do not use broad identifier sweeps that rename every matching token by text alone. Match the intended API context explicitly.
- Keep generated codemods inside the user-requested scope.
- Do not fall back to an analysis-only codemod if safe, meaningful source edits are available for the requested migration.
- An analysis-only codemod is acceptable only when there are no safe automatable edits or the user explicitly asked for analysis/reporting.
- If a hop is manual-only, explain why instead of faking automation.
- Do not present the codemod as complete while package tests are failing.

## Package-shape heuristics

- Default to `ast-grep-js` when the requested codemod is mainly JS/TS-family source edits.
- Choose `hybrid` only when shell steps, YAML ast-grep rules, or other non-JS deterministic workflow surfaces are first-class parts of the migration.
- Do not choose `hybrid` merely because the codemod also needs to generate a helper file, preview route, fixture, or README update.
- Default to a single package for a specific `from -> to` migration.
- Default to a workspace when the migration is open-ended or documented as multiple version hops.
- If official docs show intermediate hop guides, treat that as a signal for multiple codemods.
- If a migration spans multiple documented hops, keep the execution order explicit and validate each hop independently.

## Use sub-agents when the task is large

- Small, exact, one-hop codemods can stay in one agent.
- For non-trivial work, split the job into focused sub-agents:
  - research for web/docs gathering
  - codebase analysis for representative "before" examples and edge-case discovery
  - test planning for fixture-matrix definition
  - implementation against the defined fixtures
- For multi-hop upgrades, parallelize per-hop research and implementation where the guides are independent.
- Keep one coordinator responsible for the final package shape, execution order, and summary.

## Search the target codebase first

1. Inspect the target repo for real "before" examples that the codemod must transform.
2. Cluster those examples into concrete transformation patterns.
3. Record edge cases, no-op cases, and patterns that should stay unchanged.
4. Use those findings to refine the migration plan before implementation starts.

Do not rely only on abstract migration docs when the actual codebase reveals usage variants that affect transform behavior.

## Required research flow

1. Search the web first for migration guidance. Prefer official migration guides, release notes, and upgrade docs.
2. Add high-signal secondary sources only when they materially improve the plan, for example maintainer posts, changelogs, or issues clarifying edge cases.
3. Cross-check secondary sources against official docs before encoding behavior in a codemod.
4. If official docs are missing or incomplete, state that explicitly and proceed with the best available sources instead of skipping research.
5. If a hop is manual-only, record why it is manual-only and which researched changes were deemed unsafe or non-automatable.

## Scaffold guardrails

- Prefer `npx codemod@latest ...` or a verified local Codemod binary when command resolution is uncertain.
- For non-interactive scaffolding, rely on the public CLI docs or `codemod init --help` for the current required flags instead of guessing them.
- Quote multi-word registry searches.

## Multi-hop workspace execution

- For upgrade series, scaffold a workspace first, then add one codemod per hop under `codemods/<slug>/`.
- Name packages so the hop is obvious.
- If the user asked for an evergreen or "latest" migration, document the full recommended hop chain from the oldest supported entrypoint to the newest target.
- If one hop is manual-only, still keep it documented so the execution order remains complete.

## Expected package shape

- Every codemod package should have `workflow.yaml` and `codemod.yaml`.
- The default package shape is ast-grep-based, typically `js-ast-grep` with `scripts/codemod.ts` for JS/TS-family source edits.
- Add extra workflow steps only when the migration includes other deterministic structured files or truly separate safe surfaces.
- If the migration spans multiple related files but remains AST-safe, prefer staying in JSSG with `jssgTransform` or related APIs over introducing a shell step.
- Do not replace the default package shape with shell scripts unless the user explicitly asked for a shell/native workflow or there is no viable ast-grep-based path.
- In monorepos, each codemod should live under `codemods/<slug>/`.

## Test discipline

- Define the test matrix before implementing transforms.
- Base that matrix on both real repo examples and documented migration cases.
- Include representative repo-derived cases, realistic doc-derived cases, edge cases, preserve/no-op coverage, and negative cases where similar code should stay unchanged.
- If the migration scope is broad, the test matrix must be broad as well.
- During iteration, use `codemod jssg test ... --strictness loose` when formatting noise would otherwise hide semantic progress.
- Before calling the codemod complete, rerun the package's normal/default test command and ensure it is green without relying on debugging-only flags.

## Test loop expectations

- Do not implement the codemod blindly and generate tests afterward; define the expected cases first and implement against them.
- Run the test suite before presenting the result.
- If tests fail, inspect the expected-vs-actual diff and fix the codemod rather than leaving the failure unexplained.
- For multi-hop workspaces, run tests per package and keep each hop green independently.
- Do not present a codemod as complete if the tests only prove a trivial happy path.
- Do not present a codemod as complete while any package default test command is failing, including snapshot or metrics mismatches.
- Before documenting local run or validation commands in a README, verify them against the current CLI help.

## MCP usage

- Use `get_jssg_instructions` for transform/runtime guidance.
- Use `get_codemod_cli_instructions` for package/workflow/CLI semantics.
- Use `get_jssg_utils_instructions` for import helper usage.
- Use `dump_ast` and `run_jssg_tests` during iteration, not just once at startup.
- If symbol origin or cross-file references matter, enable `semantic_analysis: workspace` in the workflow.
- If Codemod MCP is unavailable, fall back to the installed `codemod` skill behavior plus current CLI help instead of inventing workflow or command semantics.

## Publish expectations

- Keep codemods on the current branch unless the user explicitly wants branch automation.
- Do not push automatically.
- Use trusted publisher/OIDC based publishing when wiring GitHub Actions.
- For multi-hop workspaces, validate every hop independently before proposing publish automation for the full series.

## If guidance conflicts

Prefer the public docs over this file.

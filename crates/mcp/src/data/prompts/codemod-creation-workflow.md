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
- If the expected Codemod MCP tools are not available in the callable tool list for the current session, stop codemod authoring and instruct the user to fix MCP visibility first.

## Default process

1. Plan the migration and search for an existing codemod before creating a new one.
2. Inspect the target codebase for representative real examples.
3. Call `get_jssg_gotchas` and `get_ast_grep_gotchas` before writing source-transform code.
4. Research the migration path before choosing package shape.
5. If registry search yields no exact package, call `scaffold_codemod_package` immediately.
6. Define the test matrix and create the initial fixtures before implementing transforms.
7. Use Codemod MCP throughout authoring, not just once at startup.
8. Keep iterating until workflow validation, package tests, and package validation pass.

## Required action loop

1. Search the registry with one or more quoted queries.
2. If there is no exact package, scaffold immediately.
3. Once the package exists, replace the starter transform, README text, and starter fixtures immediately.
4. Define the requested positive, negative, and edge fixtures before deep implementation work.
5. Implement the deterministic transform.
6. Run tests.
7. Repair failing cases using KB search and `dump_ast` before considering broader fallbacks.
8. Call `validate_codemod_package`.
9. Do not stop until validation is clean.

## Non-negotiable rules

- Use AST-based edits for code transforms.
- Default to ast-grep-based codemod packages for deterministic source transforms.
- Prefer `js-ast-grep` for JS/TS-family source edits and workflow steps for other deterministic structured files.
- Use multi-step workflows when the migration spans multiple safe transformation surfaces.
- Do not use broad identifier sweeps that rename every matching token by text alone. Match the intended API context explicitly.
- If related files remain AST-safe, keep the hop inside JSSG with `jssgTransform` or another JSSG API instead of introducing a shell step.
- For source transforms, do not use `RegExp`, `.replace`, `.replaceAll`, `.match`, `.split`, manual string parsing, or raw source-text rewriting as the primary implementation strategy.
- Minimal string operations are acceptable only for path normalization, import/module-specifier cleanup, helper or metadata formatting, and test-output parsing.
- Do not switch to shell/native scripts as the primary transformation engine unless the user explicitly asked for that implementation style or no ast-grep-based path is viable.
- Treat dependency or version manifest upgrades as part of the migration surface when the researched upgrade path requires them and the edits are deterministic.
- Keep generated codemods inside the user-requested scope.
- Do not fall back to an analysis-only codemod if safe, meaningful source edits are available for the requested migration.
- An analysis-only codemod is acceptable only when there are no safe automatable edits or the user explicitly asked for analysis/reporting.
- If a hop is manual-only, explain why instead of faking automation.
- Tests must be comprehensive relative to the user's request, not just the easiest documented example.
- Do not present the codemod as complete while package tests are failing.
- Do not stop at scaffold. Replace starter transforms, starter README text, and starter fixtures with real package content for the requested migration.

## Package-shape heuristics

- Default to `ast-grep-js` when the requested codemod is mainly JS/TS-family source edits.
- Choose `hybrid` only when shell steps, YAML ast-grep rules, or other non-JS deterministic workflow surfaces are first-class parts of the migration.
- Do not choose `hybrid` merely because the codemod also needs to generate a helper file, preview route, fixture, or README update.
- Default to a single package for one granular transform or a specific `from -> to` migration.
- Keep a single package even when that codemod supports multiple page shapes, helper-file generation, or several related deterministic file edits.
- Do not create a workspace merely because one codemod has multiple fixtures, supported route shapes, or helper files.
- Default to a workspace when the migration is open-ended or documented as multiple version hops.
- If official docs show intermediate hop guides, treat that as a signal for multiple codemods.
- If the user asked for "latest", "stay up to date", or another open-ended upgrade target, treat that as a signal for a workspace unless research proves one direct hop is the documented path.
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
4. Build the version-hop plan before implementation begins.
5. If official docs are missing or incomplete, state that explicitly and proceed with the best available sources instead of skipping research.
6. If a hop is manual-only, record why it is manual-only and which researched changes were deemed unsafe or non-automatable.

When the migration has multiple documented hop guides:

- Gather those guides in parallel when possible.
- Plan each hop separately.
- Keep the final execution order explicit in the workspace README and package descriptions.

## Scaffold guardrails

- Prefer `npx codemod@latest ...` or a verified local Codemod binary when command resolution is uncertain.
- For non-interactive scaffolding, rely on the public CLI docs or `codemod init --help` for the current required flags instead of guessing them.
- Quote multi-word registry searches.
- Treat adjacent or generic registry packages as "no exact package" unless they directly implement the requested migration.
- Prefer `scaffold_codemod_package` over hand-writing `codemod init` commands when the tool is available.
- Once registry search misses, do not continue indefinite broad research before a package exists; scaffold first, then implement against fixtures.

## Deterministic repair and escalation

- For failing deterministic cases, search the verified KB tools first and use `dump_ast` before broadening heuristics.
- Allow up to 3 deterministic repair attempts per unresolved case in a session.
- After 3 failed deterministic attempts:
  - use a narrow AI fallback only for the isolated unresolved subset, or
  - mark the case as manual follow-up when AI would not be narrow or reliable
- Do not make AI steps the primary transformation engine for the whole codemod.
- If an AI fallback also fails to stabilize the case, document it as `manual transformation needed` instead of broadening regex or manual-parsing heuristics.

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

## Validate and test

- Validate workflow/package structure before calling the codemod complete.
- Prefer `codemod workflow validate` during authoring instead of assuming the generated workflow remains valid after edits.
- Define the test matrix before implementing transforms.
- Create the initial fixtures before writing the transform so implementation is driven by expected behavior rather than post-hoc patching.
- Base that matrix on both real repo examples and documented migration cases.
- Include representative repo-derived cases, realistic doc-derived cases, edge cases, preserve/no-op coverage, negative cases where similar code should stay unchanged, and version-hop-specific cases when behavior changes between hops.
- If the migration scope is broad, the test matrix must be broad as well.
- If the package already has JSSG fixtures, extend that existing fixture set instead of inventing ad hoc standalone test files.
- During implementation and debugging, prefer the direct `codemod jssg test` command rather than only `npm test`, so failures stay visible and filterable.
- During iteration, use `codemod jssg test ... --strictness loose` when formatting noise would otherwise hide semantic progress.
- Use `--filter <case>` when isolating failures.
- Follow the documented JSSG fixture layout so snapshots remain stable and reviewable.
- Treat `metrics.json` like code snapshots: if metrics differ, refresh or fix them and rerun the package's normal/default tests before summarizing.
- Before calling the codemod complete, rerun the package's normal/default test command and ensure it is green without relying on debugging-only flags.
- Before calling the codemod complete, call `validate_codemod_package` and treat any starter-scaffold or failing-check result as blocking.

## Test loop expectations

- Do not implement the codemod blindly and generate tests afterward; define the expected cases first and implement against them.
- Run the test suite before presenting the result.
- If tests fail, inspect the expected-vs-actual diff and fix the codemod rather than leaving the failure unexplained.
- For multi-hop workspaces, run tests per package and keep each hop green independently.
- Do not present a codemod as complete if the tests only prove a trivial happy path.
- Do not present a codemod as complete while any package default test command is failing, including snapshot or metrics mismatches.
- Before documenting local run or validation commands in a README, verify them against the current CLI help.

## Package completion checklist

- After changing the codemod, inspect and update the whole package surface: `README`, `codemod.yaml`, `workflow.yaml`, tests, and package metadata.
- If you renamed the codemod or changed its described scope, update package names, README examples, and workflow references in the same change.
- If the package identity or publish surface changed, update the relevant name/version/description metadata instead of leaving stale package metadata behind.
- If the codemod uses capability-gated runtime APIs such as `fs`, `fetch`, or `child_process`, update `codemod.yaml` in the same change with the matching `capabilities` entry.

## MCP usage

- Use `get_jssg_instructions` for transform/runtime guidance.
- Use `get_jssg_gotchas` and `get_ast_grep_gotchas` before writing source-transform code.
- Use `search_jssg_knowledge` and `search_ast_grep_knowledge` when a pattern or repair strategy is unclear.
- Use `get_jssg_runtime_capabilities` when the codemod needs Node/LLRT APIs, capability-gated modules, or non-trivial multi-file JSSG work.
- Use `get_codemod_cli_instructions` for package/workflow/CLI semantics.
- Use `get_jssg_utils_instructions` for import helper usage.
- Use `scaffold_codemod_package` immediately after registry search shows there is no exact package.
- Use `validate_codemod_package` before stopping work on the package.
- Use `dump_ast` and `run_jssg_tests` during iteration, not just once at startup.
- If symbol origin or cross-file references matter, enable `semantic_analysis: workspace` in the workflow.
- If Codemod MCP is unavailable in the callable tool list, stop and tell the user to reload/restart Codex and fix Codemod MCP setup before continuing. Do not continue codemod authoring without MCP.

## Publish expectations

- Keep codemods on the current branch unless the user explicitly wants branch automation.
- Do not push automatically.
- Do not create commits automatically for codemod authoring/evaluation unless the user explicitly asked for git operations.
- Use trusted publisher/OIDC based publishing when wiring GitHub Actions.
- For multi-hop workspaces, validate every hop independently before proposing publish automation for the full series.

## If guidance conflicts

Prefer the public docs over this file.

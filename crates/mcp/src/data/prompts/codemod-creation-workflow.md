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

- Default to a single package for a specific `from -> to` migration.
- Default to a workspace when the migration is open-ended or documented as multiple version hops.
- If official docs show intermediate hop guides, treat that as a signal for multiple codemods.
- If a migration spans multiple documented hops, keep the execution order explicit and validate each hop independently.

## Test discipline

- Define the test matrix before implementing transforms.
- Base that matrix on both real repo examples and documented migration cases.
- During iteration, use `codemod jssg test ... --strictness loose` when formatting noise would otherwise hide semantic progress.
- Before calling the codemod complete, rerun the package's normal/default test command and ensure it is green without relying on debugging-only flags.

## MCP usage

- Use `get_jssg_instructions` for transform/runtime guidance.
- Use `get_codemod_cli_instructions` for package/workflow/CLI semantics.
- Use `get_jssg_utils_instructions` for import helper usage.
- Use `dump_ast` and `run_jssg_tests` during iteration, not just once at startup.
- If symbol origin or cross-file references matter, enable `semantic_analysis: workspace` in the workflow.

## If guidance conflicts

Prefer the public docs over this file.

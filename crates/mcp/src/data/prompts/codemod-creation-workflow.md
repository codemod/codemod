# Supplemental Codemod Creation Guidance

This file is a local supplement for agent workflow policy.

Public Codemod docs remain the source of truth for:
- CLI syntax
- package/workflow semantics
- JSSG syntax
- fixture layout

Use this file only for the extra agent guidance that public docs should not carry.

## Default authoring loop

1. Search the registry before deciding to create a new package.
2. Inspect 1-3 representative repo files after or alongside registry discovery.
3. If there is no exact package, scaffold immediately with direct `codemod init`.
4. Replace starter transform/README/fixtures immediately after scaffold.
5. Define positive, negative, and edge fixtures before deep implementation.
6. Implement the deterministic transform.
7. Run tests, validate the workflow, and validate the package before stopping.

## Hot-path caveats

- If Codemod MCP is missing from the callable tool list, stop and tell the user to fix MCP visibility first.
- Read `jssg-gotchas` and `ast-grep-gotchas` before writing source-transform code.
- Use `dump_ast` when the pattern shape is unclear.
- If symbol origin matters, use semantic analysis and binding-aware checks.
- In `workflow.yaml`, shell steps use `run:`, not `command:`.
- Keep JSSG/ast-grep as the primary transformation engine; use shell/native steps only when the user asked for them or no viable AST-safe path exists.
- If official migration steps require deterministic dependency, manifest, or config edits, keep those in scope instead of reducing the codemod to source-only changes.
- Do not reduce a requested migration codemod to analysis-only output when safe automatable edits exist.
- Before stopping, inspect the whole package surface and update every affected file together: `README.md`, `codemod.yaml`, `workflow.yaml`, `package.json` scripts, tests/fixtures, and any renamed paths, ids, or references. Do not churn versions by default, but do not leave stale package metadata behind after a rename or material package-surface change.
- Preserve the scaffold-selected package manager in package scripts and package-local README/development commands. Do not rewrite `yarn`/`pnpm`/`bun` packages to another runner unless the user explicitly asked.
- After a registry miss, run `codemod init` immediately. In headless/non-interactive flows, use `codemod init <path> --no-interactive` and pass only user- or task-provided flags. Do not invent `--author`, `--license`, `--description`, or `--git-repository-url`; rely on the simplified CLI defaults and publish-time auth-derived author fallback.
- Use `validate_codemod_package` before stopping.
- Do not create commits or push branches unless the user explicitly asked for git operations.

## Review-derived quality gates

- Keep the requested migration aligned across every artifact: transform logic, fixtures, `workflow.yaml`, `codemod.yaml`, README, and package metadata must all describe the same codemod.
- Replace scaffold boilerplate before finishing. Do not leave generic README text, placeholder fixture intent, or mismatched usage descriptions in place.
- Match the actual target surface in workflow metadata:
  - Use explicit `base_path`, `include`, and `exclude` globs.
  - Keep `codemod.yaml` `targets.languages` aligned with the files the codemod truly supports.
  - If the codemod targets JSX/TSX, Vue SFCs, JSON, YAML, or other non-plain-JS inputs, the workflow and fixtures must use those file types instead of generic `.js` examples.
- Preserve repository package-manager and lockfile conventions when working inside an existing monorepo. Do not introduce ad hoc dependency ranges or unrelated lockfile churn.
- Treat fixture quality as a release gate. Cover realistic positive cases, edge cases, preserve/no-op cases, and negative cases where similar code must stay unchanged.
- When a codemod makes no changes for a file, prefer returning `null` instead of rewriting the full source unchanged.
- Before finishing, do a consistency pass for trailing whitespace, README command correctness, workflow command correctness, and other avoidable review noise.

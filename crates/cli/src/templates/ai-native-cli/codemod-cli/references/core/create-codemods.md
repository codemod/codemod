# Codemod CLI Core: Create Codemods

Use this guide when the task is to create or improve codemods, not just run them.

## Default workflow

1. Plan the migration before touching files.
2. Search the target codebase for representative "before" examples before choosing package shape.
3. Research the migration path on the web before choosing package shape.
4. Scaffold with `codemod init`.
5. Use Codemod MCP while authoring the codemod.
6. Generate tests that cover the discovered scope.
7. Run the codemod test suite and fix failures before finishing.
8. Iterate on tricky cases before publishing.

## Hard rules

- Use AST-based edits for JS/TS code transforms. Do not implement JS/TS codemods as raw source-string replacement or regex replacement over the full file text.
- If a code change cannot be encoded safely with AST tooling, document it as a manual step instead of shipping a brittle transform.
- A manual-only hop is acceptable only when the research shows there is no safe, meaningful automatable source change for that hop.
- Tests must be comprehensive relative to the user's request, not just the easiest documented example.
- README command examples must be checked against the current Codemod CLI help before you present them.

## Use sub-agents when the task is large

- Small, exact, one-hop codemods can stay in one agent.
- For non-trivial work, split the job into focused sub-agents:
  - research agent for web/docs gathering,
  - codebase analysis agent for "before" examples and edge-case discovery,
  - implementation agent for the transform,
  - test agent for snapshot generation and verification.
- For multi-hop upgrades, use one coordinator plus per-hop research and implementation/test agents when parallel work is possible.
- Keep the coordinator responsible for the final package shape, execution order, and summary.

## Decide between single package and workspace

- Default to a single codemod when the user gives an exact source and target version.
- Default to a workspace when the user asks to "upgrade to latest", "stay up to date", or otherwise leaves the version range open-ended.
- Use the codebase examples and web research together when making this decision; do not decide from docs alone if the target repo shows materially different patterns.
- Before deciding, inspect official migration docs, changelogs, or upgrade guides and determine whether the migration is documented as sequential version hops.
- If the docs show separate upgrade guides for intermediate versions, create a workspace and generate one codemod per documented hop.
- If the docs show one direct path with no intermediate hops, keep a single package unless the user explicitly wants a monorepo.

## Search the target codebase first

1. Inspect the current repo for real "before" examples that the codemod must transform.
2. Cluster those examples into concrete transformation patterns.
3. Record edge cases, no-op cases, and patterns that should be left unchanged.
4. Use those findings to refine the migration plan before implementation starts.

The agent should not rely only on abstract migration docs when the actual codebase reveals usage variants that affect transform behavior.

## Required research flow

1. Search the web first for migration guidance. Prefer the package's official migration guide, release notes, or upgrade docs, but also collect other credible sources when they add missing context, examples, edge cases, or ecosystem-specific gotchas.
2. Build a version-hop plan before scaffolding anything.
3. Record the supported hop order, breaking changes per hop, and any steps that are manual-only.
4. Only after the hop plan is stable, choose `codemod init` shape and start implementation.

When researching:

- Treat official docs as the primary source of truth when they exist.
- Add high-signal secondary sources when they materially improve the plan, for example framework migration blog posts, maintainer release notes, package changelogs, GitHub issues, or well-maintained upgrade guides.
- Cross-check secondary sources against official docs before encoding behavior in a codemod.
- If official docs are missing or incomplete, state that explicitly and base the plan on the best available sources instead of skipping web research.
- If you decide a hop is manual-only, record why it is manual-only and which researched changes were deemed unsafe or non-automatable.

When the migration has multiple independent hop guides:

- Gather those guides in parallel when possible.
- Gather supporting secondary sources in parallel when they help explain edge cases for specific hops.
- Plan each hop separately.
- Keep the final execution order explicit in the workspace README and package descriptions.

Example: if official docs expose separate guides such as `before-v5`, `v5-to-v6`, `v6-to-v7`, and `v7-to-v8`, treat that as a workspace migration series rather than a single codemod.

## Scaffold

- Interactive:
  - `codemod init`
- Non-interactive jssg:
  - `codemod init my-codemod --project-type ast-grep-js --language typescript --package-manager npm --description "Example codemod" --author "Your Name" --license MIT --no-interactive`
- Non-interactive workflow + skill:
  - `codemod init my-codemod --project-type ast-grep-js --with-skill --language typescript --package-manager npm --description "Example codemod" --author "Your Name" --license MIT --no-interactive`
- Non-interactive skill-only:
  - `codemod init my-codemod --skill --language typescript --description "Example codemod skill" --author "Your Name" --license MIT --no-interactive`
- Monorepo workspace:
  - `codemod init my-codemod-repo --workspace --with-skill --project-type ast-grep-js --language typescript --package-manager npm --description "Example codemod" --author "Your Name" --license MIT --no-interactive`

## Multi-hop workspace execution

- For upgrade series, scaffold a workspace first, then add one codemod per hop under `codemods/<slug>/`.
- Name packages so the hop is obvious, for example `react-native-sentry-v5-to-v6`.
- If the user asked for an evergreen or "latest" migration, the workspace should describe the full recommended hop chain from the oldest supported entrypoint to the newest supported target.
- If one hop is manual-only, still keep it documented in the workspace so the execution order remains complete.

## Codemod MCP guidance

- Use Codemod MCP when you need jssg instructions or deeper package-authoring help.
- Call `get_jssg_instructions` before writing non-trivial jssg transforms.
- When migration patterns depend on symbol origin or cross-file references, use semantic analysis.
- Enable `semantic_analysis: workspace` in the workflow when symbol definition or reference checks matter.
- Prefer AST-targeted edits through jssg patterns and semantic analysis.

## Expected package shape

- Every codemod package should have `workflow.yaml` and `codemod.yaml`.
- Workflow-capable packages usually include `scripts/codemod.ts` and tests.
- Skill-capable packages should include authored skill files under `agents/skill/<skill-name>/`.
- In monorepos, each codemod should live under `codemods/<slug>/`.

## Validate and test

- Validate workflow/package structure:
  - `codemod workflow validate -w codemods/<slug>/workflow.yaml`
- Run jssg tests from the package directory:
  - `npm test`
- Create snapshot-style test fixtures that cover the actual discovered scope, following the documented JSSG test layout:
  - `tests/<case>/input.*`
  - `tests/<case>/expected.*`
- Include:
  - representative repo-derived cases,
  - realistic cases from migration docs or release notes when the repo does not expose enough examples,
  - edge cases,
  - preserve/no-op cases,
  - negative cases where similar code should stay unchanged,
  - any version-hop-specific cases that change behavior.
- The fixture set should cover the migration scope the user asked for. If the request is broad, the test matrix must be broad as well.
- As a minimum, each non-trivial hop should include:
  - one realistic/doc-derived case,
  - one edge case,
  - one preserve/no-op case,
  - one negative case,
  - and one repo-derived case when the target repo exposes relevant examples.
- Run the codemod test suite after implementation and fix the codemod or the test fixture set until the suite passes.
- For local verification against a repo:
  - `codemod workflow run -w codemods/<slug>/workflow.yaml --target <repo-path>`

## Test loop expectations

- After writing or updating the codemod, run the test suite before presenting the result.
- If tests fail, debug and fix the codemod rather than leaving the failure unexplained.
- If the migration scope is broad, keep expanding the tests until the discovered codebase patterns are covered.
- For multi-hop workspaces, run tests per package and keep each hop green independently.
- Do not present a codemod as complete if the tests only prove a trivial happy path.
- Before documenting local run or validation commands in a README, verify them against `codemod --help`, `codemod workflow --help`, or the relevant subcommand help.

## Publish expectations

- Keep codemods on the current branch unless the user explicitly wants branch automation.
- Do not push automatically.
- Use trusted publisher/OIDC based publishing when wiring GitHub Actions.
- If the repository is a maintainer monorepo, load `references/core/maintainer-monorepo.md`.
- For multi-hop workspaces, validate every hop independently before proposing publish automation for the full series.

# Codemod CLI Core: Create Codemods

Use this guide when the task is to create or improve codemods, not just run them.

## Default workflow

1. Plan the migration before touching files.
2. Research the official migration path before choosing package shape.
3. Scaffold with `codemod init`.
4. Use Codemod MCP while authoring the codemod.
5. Validate the generated workflow and run codemod tests.
6. Iterate on tricky cases before publishing.

## Decide between single package and workspace

- Default to a single codemod when the user gives an exact source and target version.
- Default to a workspace when the user asks to "upgrade to latest", "stay up to date", or otherwise leaves the version range open-ended.
- Before deciding, inspect official migration docs, changelogs, or upgrade guides and determine whether the migration is documented as sequential version hops.
- If the docs show separate upgrade guides for intermediate versions, create a workspace and generate one codemod per documented hop.
- If the docs show one direct path with no intermediate hops, keep a single package unless the user explicitly wants a monorepo.

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
- For local verification against a repo:
  - `codemod workflow run -w codemods/<slug>/workflow.yaml --target <repo-path>`

## Publish expectations

- Keep codemods on the current branch unless the user explicitly wants branch automation.
- Do not push automatically.
- Use trusted publisher/OIDC based publishing when wiring GitHub Actions.
- If the repository is a maintainer monorepo, load `references/core/maintainer-monorepo.md`.
- For multi-hop workspaces, validate every hop independently before proposing publish automation for the full series.

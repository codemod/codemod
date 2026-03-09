# Codemod CLI Core: Create Codemods

Use this guide when the task is to create or improve codemods, not just run them.

## Default workflow

1. Plan the migration before touching files.
2. Scaffold with `codemod init`.
3. Use Codemod MCP while authoring the codemod.
4. Validate the generated workflow and run codemod tests.
5. Iterate on tricky cases before publishing.

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

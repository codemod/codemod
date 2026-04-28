# Codemod Maintainer Monorepo

Use this guide when setting up or maintaining a codemod monorepo for an open-source project, framework, SDK, or organization.

## Repository conventions

- Keep each codemod under `codemods/<slug>/`.
- Each package should own its own `workflow.yaml`, `codemod.yaml`, scripts, tests, and optional skill files.
- The workspace root should hold shared files such as `package.json`, `.gitignore`, publish workflow, and maintainer docs.
- When a migration is documented as sequential version hops, represent each hop as its own package instead of collapsing the series into one codemod.

## One-time maintainer setup

1. Create the codemod repository in the target organization.
2. Sign in to Codemod with the GitHub account that will manage publishing.
3. Install the Codemod GitHub app for the repository.
4. Configure trusted publisher/OIDC so GitHub Actions can publish without a long-lived token.
5. Ensure published package names use the intended organization scope whenever the project has one.

## Scope and publishing guidance

- If the project already publishes under an org scope, codemod package names should match that scope.
- Scoped package names make registry filtering and ownership clearer for maintainers and users.
- If the project is not scoped yet, call out that the maintainers should reserve a scope before publishing broadly.

## Agent workflow for new codemods

- Start with `codemod init --workspace` when the repository does not exist yet.
- Add the first codemod during workspace init so the repo starts with a valid package under `codemods/`.
- If the user asks for a migration to the latest supported version and official docs show intermediate hops, plan the full series first and then add one package per hop.
- Search the target codebase for real examples before finalizing hop boundaries or test scope.
- Use Codemod MCP while implementing complex codemods, especially when semantic analysis is needed.
- Validate and test each codemod from its package directory before proposing publish automation.
- Require AST-based transforms for JS/TS code edits across the workspace. If a hop cannot be automated safely with AST tooling, mark it manual-only and explain why.

## Version-hop series guidance

- Prefer official migration docs and release notes when deciding the hop boundaries.
- A version-hop workspace should make the execution order obvious from package names, README guidance, and package descriptions.
- Research each documented hop separately; do that in parallel when the hop guides are independent and the source material is already known.
- Split large workspace efforts across focused sub-agents so research, repo analysis, implementation, and testing do not share one bloated context.
- Keep any manual checkpoints in the workspace documentation even if they are not automated by a codemod.
- For any manual-only hop, keep a concise rationale in the package README so maintainers know why it is not automated.
- If the user later narrows the request to a specific `from -> to` pair, it is acceptable to create or expose only the relevant package instead of the full series.

## Repo-level expectations

- Encourage maintainers to add a root `CONTRIBUTING.md` with review, testing, and release standards.
- Keep the root README focused on maintainer setup and how users can run codemods.
- For version-hop workspaces, the root README should describe the supported upgrade chain and the recommended package execution order.
- Treat publish automation as repo-level infrastructure; keep codemod logic inside the package directories.
- Keep tests close to each codemod package and ensure every hop has its own fixture coverage and green test run before the workspace is considered complete.
- Test coverage should reflect the real migration scope for each hop, including realistic/doc-derived cases, representative repo examples when available, edge cases, preserve/no-op coverage, and negative cases.
- Verify README command examples against the current CLI help before treating the workspace docs as complete.
- Enforce package-level consistency checks during review:
  - The transform, fixtures, README, workflow file, and `codemod.yaml` must all describe the same migration.
  - Workflow `include`/`exclude` globs and `targets.languages` must match the actual file types in scope.
  - Reuse the repository's package-manager and lockfile conventions instead of adding ad hoc dependency ranges or unrelated lockfile churn.
  - Remove avoidable hygiene noise such as trailing whitespace or stale scaffold text before opening a PR.

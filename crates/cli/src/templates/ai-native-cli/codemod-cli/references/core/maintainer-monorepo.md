# Codemod CLI Core: Maintainer Monorepo

Use this guide when the user is setting up or maintaining a codemod monorepo for an open-source project, framework, SDK, or organization.

## Repository conventions

- Keep each codemod under `codemods/<slug>/`.
- Each package should own its own `workflow.yaml`, `codemod.yaml`, scripts, tests, and optional skill files.
- The workspace root should hold shared files such as `package.json`, `.gitignore`, publish workflow, and maintainer docs.

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
- Use Codemod MCP while implementing complex codemods, especially when semantic analysis is needed.
- Validate and test each codemod from its package directory before proposing publish automation.

## Repo-level expectations

- Encourage maintainers to add a root `CONTRIBUTING.md` with review, testing, and release standards.
- Keep the root README focused on maintainer setup and how users can run codemods.
- Treat publish automation as repo-level infrastructure; keep codemod logic inside the package directories.

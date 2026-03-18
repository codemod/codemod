# {title}

Official codemods for {org_label} to help users adopt new features and handle breaking changes with less manual work.

Community contributions are welcome. Use this repository to create, validate, and publish codemods from a shared monorepo.

## One-time setup

1. Create this codemod repository in {org_label}.
2. Sign in to [Codemod](https://app.codemod.com) with your GitHub account.
3. Install the Codemod GitHub app for this repository so publishes can be associated with your organization.
4. Configure a [trusted publisher](https://docs.codemod.com) in Codemod so GitHub Actions can publish with OIDC.
5. {scope_guidance}

Use [Codemod MCP](https://docs.codemod.com/model-context-protocol) and `npx codemod init` to create new codemods from this monorepo.

## Repository layout

Each codemod lives under `codemods/<slug>/`.

```text
codemods/<slug>/
  workflow.yaml
  codemod.yaml
  scripts/
  tests/
```

Keep each codemod self-contained so maintainers can validate and publish packages independently.

## Creating codemods

- Scaffold new codemods with `npx codemod init`.
- Use Codemod MCP when creating or refining codemods, especially when symbol definitions or cross-file references matter.
- Validate package workflows with `npx codemod workflow validate codemods/<slug>/workflow.yaml`.
- Run package tests from the codemod directory before publishing.

## Running codemods

> [!CAUTION]
> Codemods modify code. Run them only on Git-tracked files, and commit or stash changes first.

### From the registry

```bash
npx codemod <codemod-name>
```

### From source

```bash
npx codemod workflow run -w codemods/<slug>/workflow.yaml
```

By default, codemods run in the current folder. Add `--target /path/to/repo` to run elsewhere.

## Publishing and contribution guidance

- Use the generated GitHub Actions workflow to publish after review and merge.
- Add a `CONTRIBUTING.md` in this repository to document review, testing, and release expectations for contributors.
- See the [Codemod docs](https://go.codemod.com/docs) for CLI and publishing details.

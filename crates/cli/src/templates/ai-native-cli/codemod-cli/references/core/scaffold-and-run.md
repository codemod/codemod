# Codemod CLI Core: Run and Validate

## Validate Before Running

- Validate workflow schema and references:
  - `codemod workflow validate -w my-codemod/workflow.yaml`

## Apply a Local Workflow Codemod

- Run a local workflow package:
  - `codemod workflow run -w my-codemod --target <repo-path>`
- Pass workflow parameters:
  - `codemod workflow run -w my-codemod --target <repo-path> --param strict=true`

## Apply a Registry Codemod

- Run published package explicitly:
  - `codemod run <package-name> --target <repo-path>`
- Run published package via implicit package mode:
  - `codemod <package-name> --target <repo-path>`

For codemod creation workflows, use `references/core/create-codemods.md`.

## Direct jssg Development Loop

- Run a local jssg transform:
  - `codemod jssg run ./codemod.ts --target <repo-path> --language typescript`
- Run fixture tests:
  - `codemod jssg test ./codemod.ts --language typescript`
- Show files where selector applies:
  - `codemod jssg list-applicable ./codemod.ts --target <repo-path> --language typescript`

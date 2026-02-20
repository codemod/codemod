# Codemod CLI Core: Scaffold and Run

Use this guide for creating codemods and applying them.

## Scaffold a New Codemod

- Interactive setup:
  - `codemod init`
- Non-interactive setup:
  - `codemod init my-codemod --project-type ast-grep-js --language typescript --no-interactive`
- Force overwrite existing files:
  - `codemod init my-codemod --force`

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

## Direct jssg Development Loop

- Run a local jssg transform:
  - `codemod jssg run ./codemod.ts --target <repo-path> --language typescript`
- Run fixture tests:
  - `codemod jssg test ./codemod.ts --language typescript`
- Show files where selector applies:
  - `codemod jssg list-applicable ./codemod.ts --target <repo-path> --language typescript`

## AI-Native Extension

Use `agent` and `tcs` when orchestration/routing is needed:
- `codemod agent run "<intent>" --harness auto --format json`
- `codemod tcs install <tcs-id> --harness auto --project`

If `agent run` returns `insufficient_metadata` or `no_candidates`, use direct CLI flow:
- `codemod search "<migration>" --format json`
- `codemod run <package-name> --dry-run --target <repo-path>`
- `codemod run <package-name> --target <repo-path>`

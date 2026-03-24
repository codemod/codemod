# Fallback Codemod CLI Guidance

This file is a fallback used only when the public Codemod docs cannot be fetched at runtime.

The public docs are the source of truth for:
- CLI usage
- package structure
- workflow authoring
- sharding
- Campaign/cloud git automation semantics

When the public docs are available, prefer them over this file.

## What this fallback is for

Use this fallback to keep agents on the right path when the public docs are unavailable:
- find existing codemods before creating new ones
- scaffold packages with `codemod init`
- validate workflows with `codemod workflow validate`
- dry-run before apply
- defer to subcommand help when exact flags are uncertain

## Safe defaults

- For migration, upgrade, update, or deprecation-rollout requests, run `codemod search` before proposing a custom codemod.
- For local packages, validate first:
  - `codemod workflow validate -w <workflow-or-package-path>`
- Dry-run before apply:
  - `codemod workflow run -w <workflow-or-package-path> --target <repo-path> --dry-run`
  - `codemod run <package-name> --target <repo-path> --dry-run`
- Prefer non-interactive execution for agents:
  - add `--no-interactive` where supported

## Minimal workflow reminders

- Use `run:` for shell steps, not `command:`.
- Put transforms under `nodes[].steps[]`.
- `js-ast-grep`, `ast-grep`, `ai`, `codemod`, `run`, `install-skill`, and `shard` are step actions.
- In Campaign/cloud runs, workflow nodes may use `branch_name`, `pull_request`, and step-level `commit`.
- For exact workflow semantics, rely on the public workflow docs whenever possible.

## Minimal command reminders

- Search:
  - `codemod search <query>`
- Scaffold:
  - `codemod init`
- Validate:
  - `codemod workflow validate -w <path>`
- Run local workflow/package:
  - `codemod workflow run -w <path> --target <repo-path>`
- Run registry package:
  - `codemod run <package-name> --target <repo-path>`
- Show command help:
  - `codemod --help`
  - `codemod workflow --help`
  - `codemod jssg --help`

## If syntax is uncertain

Prefer the CLI help output over guessing:
- `codemod --help`
- `codemod workflow --help`
- `codemod jssg --help`

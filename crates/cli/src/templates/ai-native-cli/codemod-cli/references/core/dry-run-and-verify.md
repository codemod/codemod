# Codemod CLI Core: Dry Run and Verify

Use this sequence to minimize risk before applying edits.

## Recommended Safe Sequence

1. Read the selected package's README/docs and capture prerequisites, config knobs, and known limitations.
2. Validate workflow or package assumptions.
3. Apply documented prerequisites and repository setup before execution.
4. Execute dry run.
5. Inspect summary/diff output.
6. Apply for real only after review.

## Package Prerequisites

Before running a registry package:

- read its README and any linked migration/setup guide,
- create the recommended branch or backup state if the docs call for it,
- install/remove/update dependencies or baseline framework config if the docs require that first,
- create any package-specific config file that keeps runs deterministic,
- note which areas the package explicitly leaves for manual follow-up.

## Validate

- Validate local workflow:
  - `codemod workflow validate -w my-codemod/workflow.yaml`

## Dry Run Commands

- Local workflow dry run:
  - `codemod workflow run -w my-codemod --target <repo-path> --dry-run`
- Registry package dry run:
  - `codemod run <package-name> --target <repo-path> --dry-run`
- jssg dry run:
  - `codemod jssg run ./codemod.ts --target <repo-path> --language typescript --dry-run`

## Review Aids

- Disable color in dry-run diffs for log parsing:
  - `codemod workflow run -w my-codemod --target <repo-path> --dry-run --no-color`
- Use JSON output on discovery commands when another tool parses results:
  - `codemod search react --format json`

## Apply After Approval

- Local workflow apply:
  - `codemod workflow run -w my-codemod --target <repo-path>`
- Registry package apply:
  - `codemod run <package-name> --target <repo-path>`

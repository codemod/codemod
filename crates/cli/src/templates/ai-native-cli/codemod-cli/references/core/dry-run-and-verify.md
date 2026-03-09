# Codemod CLI Core: Dry Run and Verify

Use this sequence to minimize risk before applying edits.

## Recommended Safe Sequence

1. Validate workflow or package assumptions.
2. Execute dry run.
3. Inspect summary/diff output.
4. Apply for real only after review.

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

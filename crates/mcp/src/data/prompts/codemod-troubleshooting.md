# Codemod CLI Troubleshooting

Use these checks when commands fail or produce unexpected output.

## Agent-Safe Defaults

For agents/automation, prefer non-interactive execution:
- add `--no-interactive` to `codemod workflow run` and `codemod run`.

## Dirty Git Tree Blocking Execution

Symptom:
- command aborts because working tree is dirty.

Fix:
- review and commit/stash changes, or explicitly allow dirty state:
  - `codemod workflow run -w my-codemod --target <repo-path> --allow-dirty`
  - `codemod run <package-name> --target <repo-path> --allow-dirty`

## Parameter Parsing Errors

Symptom:
- parse failure for params.

Fix:
- pass each parameter as one `key=value` token:
  - `codemod workflow run -w my-codemod --param strict=true --param format=esm`

## Capability/Permission Failures

Symptom:
- transform needs filesystem, network, or child process capability.

Fix:
- enable required capability flags:
  - `--allow-fs`
  - `--allow-fetch`
  - `--allow-child-process`
- for automation, combine with:
  - `--no-interactive`

## Registry/Auth Failures

Symptom:
- package resolution/search/publish fails with auth errors.

Fix:
- check current auth:
  - `codemod whoami`
- login:
  - `codemod login`
- logout/reset:
  - `codemod logout --all`

## Search Returns No Useful Results

Fix:
- broaden query text and increase result size:
  - `codemod search migration --size 50`
  - `codemod search "jest vitest migration" --size 50`

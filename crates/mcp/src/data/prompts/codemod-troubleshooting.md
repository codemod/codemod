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
- transform fails with `err.code === "EACCES"` when reading/writing outside the target directory, or `fetch is not defined`, or `Cannot find module 'child_process'`.

Fix:
- The default sandboxed `fs` already allows reads/writes inside the target directory. If the codemod only touches files inside the repo, no flag is needed — investigate the path instead.
- If the codemod genuinely needs to access paths outside `target_dir`, network, or subprocesses, enable the matching flag:
  - `--allow-fs` — swaps the sandboxed fs for the unrestricted real-disk fs (removes the `target_dir` prefix check).
  - `--allow-fetch` — exposes the `fetch` global.
  - `--allow-child-process` — exposes the `child_process` module.
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

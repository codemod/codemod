# Codemod Troubleshooting Supplement

This file is a compact local supplement.

Use the public Codemod docs and current CLI help for canonical command behavior.

## Codemod MCP missing in Codex

Symptom:
- the installed `codemod` skill references Codemod MCP tools, but those tools are not in the callable tool list

Fix:
- reload or restart the Codex session/workspace after `codemod ai` install
- verify the workspace MCP config is present
- do not continue codemod authoring until Codemod MCP tools are visible

## Dirty tree blocking workflow runs

Symptom:
- workflow dry-run/apply stops because the target repo is dirty

Fix:
- clean/stash the target repo, or use `--allow-dirty` when dirty-state execution is intentional

## Shell step schema mistake

Symptom:
- `workflow validate` fails because a shell step was written with `command:` instead of the workflow schema key

Fix:
- use `run:` for shell command steps in `workflow.yaml`
- do not invent `command:` as a workflow step field

## Capability/permission failures

Symptom:
- a transform fails with `err.code === "EACCES"` when reading/writing outside the target directory, `fetch is not defined`, or `Cannot find module 'child_process'`

Fix:
- the default sandboxed `fs` allows reads/writes inside the target directory; if the codemod only touches files inside the repo, investigate the path before adding capability flags
- if the codemod genuinely needs paths outside `target_dir`, network, or subprocesses, enable the matching flag: `--allow-fs`, `--allow-fetch`, or `--allow-child-process`
- for automation, combine runtime flags with `--no-interactive`

## Registry/Auth Failures

Symptom:
- package resolution, search, or publish fails with auth errors

Fix:
- check current auth with `codemod whoami`
- login with `codemod login`
- logout or reset with `codemod logout --all`

## Search Returns No Useful Results

Fix:
- broaden query text and increase result size, for example `codemod search migration --size 50`
- quote multi-word queries, for example `codemod search "jest vitest migration" --size 50`

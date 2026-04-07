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

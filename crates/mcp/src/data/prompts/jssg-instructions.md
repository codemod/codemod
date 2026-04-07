# JSSG Fallback

This file is only a compact fallback.

Public Codemod docs are the source of truth for:
- JSSG syntax and APIs
- testing layout and commands
- semantic analysis
- advanced/multi-file helpers

When public docs are available, prefer them over this file.

Agent-only reminders:
- Stay AST-first for source transforms; do not use regex or raw source-text rewriting as the primary implementation.
- Use `dump_ast` before broadening heuristics.
- If symbol origin matters, enable semantic analysis and add binding-aware guards instead of sweeping by text alone.
- Return `null` when no change is needed, and commit collected edits from the root node.
- If runtime-gated APIs such as `fs`, `fetch`, or `child_process` are used, update `codemod.yaml` capabilities in the same change.

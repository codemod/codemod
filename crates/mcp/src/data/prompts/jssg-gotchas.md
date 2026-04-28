# JSSG Hot-Path Gotchas

- Stay AST-first for source transforms; do not fall back to regex or raw source-text rewriting.
- Use `dump_ast` before broadening heuristics or changing pattern shape.
- If symbol origin matters, enable semantic analysis and use binding-aware checks.
- Return `null` when no file change is needed.
- Commit collected edits from the root node rather than rebuilding whole files as text.
- If runtime-gated APIs such as `fs`, `fetch`, or `child_process` are used, update `codemod.yaml` capabilities in the same change.

# JSSG Hot-Path Gotchas

- Stay AST-first for source transforms; do not fall back to regex or raw source-text rewriting.
- Use `dump_ast` before broadening heuristics or changing pattern shape.
- If symbol origin matters, enable semantic analysis and use binding-aware checks.
- If JS/TS output is reconstructed as strings, explicitly check:
  - precedence and associativity
  - evaluation order / side effects
  - comment preservation
  - JSX spread/attribute ordering
  - truthy/falsy value semantics for migrated props/options
- Return `null` when no file change is needed.
- Commit collected edits from the root node rather than rebuilding whole files as text.
- If runtime-gated APIs such as `fs`, `fetch`, or `child_process` are used, update `codemod.yaml` capabilities in the same change.

# ast-grep Hot-Path Gotchas

- Patterns match AST shape, not raw text.
- Meta-variables capture whole nodes, not partial identifier substrings.
- Use `dump_ast` when a pattern does not match the expected shape.
- Do not rename or match identifiers by broad text sweeps alone.
- If symbol origin matters, use semantic analysis and binding-aware checks instead of text-only matching.

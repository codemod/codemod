# Python Semantic Provider Rules

This crate implements Python semantic analysis through Ruff/ty internals.

- Treat Ruff and Salsa APIs as version-sensitive. Keep adapter code narrow and covered by tests.
- Keep path and virtual-filesystem behavior consistent with `language-core`.
- Prefer fixtures that exercise imports, aliases, package roots, and ambiguous references.
- Validate with `cargo test -p language-python` and sandbox Python semantic integration tests when
  provider behavior changes.

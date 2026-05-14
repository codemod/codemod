# Semantic Provider Rules

This crate defines core traits, types, errors, filesystem abstractions, and the noop provider for
semantic analysis.

- Keep provider traits language-neutral and minimal. Add language-specific behavior in the provider
  crate, not here.
- Preserve stable request/response semantics for goto-definition, find-references, and related
  provider operations.
- Changes here usually require checking `language-javascript`, `language-python`,
  `semantic-factory`, and sandbox semantic integration tests.

Validation: `cargo test -p language-core -p language-javascript -p language-python -p semantic-factory`

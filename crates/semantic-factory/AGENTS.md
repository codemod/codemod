# Semantic Factory Rules

This crate selects and lazily constructs semantic providers.

- Keep provider selection predictable and explicit. Do not silently fall back to a different language
  provider when a requested provider fails to initialize.
- Preserve lazy initialization and error reporting; initialization failures should be actionable.
- Provider additions must update factory config, tests, and any sandbox/CLI language routing that
  exposes the provider.

Validation: `cargo test -p semantic-factory`

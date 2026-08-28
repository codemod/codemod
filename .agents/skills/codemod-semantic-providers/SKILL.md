---
name: codemod-semantic-providers
description: Use when changing Codemod semantic analysis providers, language-core traits, JavaScript/TypeScript provider behavior, Python provider behavior, semantic-factory routing, tree-sitter loader behavior, goto-definition, find-references, or semantic integration tests.
---

# Codemod Semantic Providers

## Key Areas

- Shared traits and types: `crates/language-core`
- JavaScript/TypeScript provider: `crates/language-javascript`
- Python provider: `crates/language-python`
- Provider selection: `crates/semantic-factory`
- Parser loading: `crates/tree-sitter-loader`
- Integration tests: `crates/codemod-sandbox/tests/integration/semantic`

## Rules

- Keep `language-core` language-neutral.
- Preserve provider contracts for goto-definition, find-references, file identity, and virtual
  filesystem behavior.
- Do not silently fall back to a different semantic provider when initialization fails.
- Add fixtures for imports, aliases, workspace roots, ambiguous references, and false positives when
  behavior changes.
- JavaScript provider changes should consider OXC resolver and Yarn PnP behavior.
- Python provider changes should keep Ruff/ty integration narrow and version-aware.
- Do not print directly to stdout/stderr from provider code. Return diagnostics or errors to callers
  so `crates/cli` can preserve TUI and JSONL output modes.

## Validation

- Provider crates:
  `cargo test -p language-core -p language-javascript -p language-python -p semantic-factory`
- Sandbox semantic integration: `cargo test -p codemod-sandbox semantic`
- Factory only: `cargo test -p semantic-factory`

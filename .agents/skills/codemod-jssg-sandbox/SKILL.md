---
name: codemod-jssg-sandbox
description: Use when working on JSSG, Codemod's JavaScript/TypeScript sandbox, ast-grep bindings, QuickJS runtime, WASM/native sandbox behavior, runtime module shims, sandbox capabilities, packages/jssg-types, packages/jssg-utils, or codemod author APIs.
---

# Codemod JSSG Sandbox

## Key Areas

- Rust sandbox: `crates/codemod-sandbox/src`
- TypeScript runtime glue: `crates/codemod-sandbox/js`
- Sandbox package tests: `crates/codemod-sandbox/tests`
- Type declarations: `packages/jssg-types/src`
- Author utilities: `packages/jssg-utils/src`

## Rules

- Do not hand-edit generated artifacts: `crates/codemod-sandbox/dist`, `js/factory.js`, or
  `sandbox.wasm`.
- Keep native and WASM behavior aligned for runtime modules, filesystem access, ast-grep bindings,
  and capabilities.
- Capability changes often need Rust capability updates, TS exports, and declaration updates.
- Keep sandbox isolation explicit. Treat filesystem, process, network, and module access as security
  boundaries.
- Do not print directly to stdout/stderr from sandbox internals or JSSG packages. Return output,
  diagnostics, metrics, and logs to callers so `crates/cli` can route TUI, text, and JSONL modes.
- JSSG utility functions should produce predictable edits and clear types for codemod authors.

## Validation

- Sandbox Rust: `cargo test -p codemod-sandbox`
- Sandbox package: `pnpm --filter @codemod.com/codemod-sandbox test`
- Sandbox build: `pnpm --filter @codemod.com/codemod-sandbox build`
- JSSG utils: `pnpm --filter @jssg/utils test`
- Type declarations: `pnpm --filter @codemod.com/jssg-types typecheck`

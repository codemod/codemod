# JSSG Sandbox Rules

This crate owns the JavaScript/TypeScript sandbox used by JSSG, ast-grep bindings, QuickJS runtime
integration, WASM/native builds, runtime module declarations, and sandbox package artifacts.

## Source Boundaries

- Rust runtime code lives in `src`; TypeScript package/runtime glue lives in `js`; generated package
  output lives in `dist/js` and should not be edited by hand.
- Do not edit `js/factory.js` by hand; it is generated and ignored by git.
- Keep native and WASM behavior aligned when changing sandbox execution, filesystem abstractions,
  ast-grep bindings, or runtime modules.
- Capability changes must stay synchronized across Rust capability definitions, TypeScript exports,
  and `packages/jssg-types` declarations when applicable.
- Preserve sandbox isolation. Be explicit about filesystem, network, process, and runtime-module
  access.

## Validation

- Rust sandbox tests: `cargo test -p codemod-sandbox`
- Package build: `pnpm --filter @codemod.com/codemod-sandbox build`
- Package tests: `pnpm --filter @codemod.com/codemod-sandbox test`
- Semantic integration focus: `cargo test -p codemod-sandbox semantic`

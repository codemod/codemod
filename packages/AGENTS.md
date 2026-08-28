# TypeScript Package Rules

This subtree contains shared TypeScript packages used by codemod authors and internal packages.

## Package Contracts

- Public packages must preserve export paths declared in `package.json`.
- `packages/jssg-types` is declaration-first; do not introduce runtime code there unless the package
  contract changes intentionally.
- `packages/jssg-utils` helpers should return predictable ast-grep edits and avoid hidden global
  state.
- Use the shared `@codemod.com/tsconfig` configs unless a package has a concrete reason to diverge.
- Keep type declarations synchronized with runtime modules in `crates/codemod-sandbox` when changing
  JSSG APIs.

## Validation

- JSSG utils: `pnpm --filter @jssg/utils test`
- JSSG utils typecheck: `pnpm --filter @jssg/utils typecheck`
- JSSG types typecheck: `pnpm --filter @codemod.com/jssg-types typecheck`
- Repo lint/format: `pnpm lint` and `pnpm format`

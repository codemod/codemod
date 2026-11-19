# TypeScript Package Rules

This subtree contains shared TypeScript packages used by codemod authors and internal packages.

## Package Contracts

- Public packages must preserve export paths declared in `package.json`.
- Type declarations for codemod authors live in `crates/cli/npm/types` and ship in the `codemod`
  npm package. `packages/jssg-types` only mirrors them (`scripts/sync.mjs`, run on pack); do not
  edit its generated `src/`. The declarations are declaration-first; do not introduce runtime code unless the package
  contract changes intentionally.
- `packages/jssg-utils` helpers should return predictable ast-grep edits and avoid hidden global
  state.
- A new or changed export under `packages/jssg-utils/src/*/exports/` updates the matching
  language page under `docs/community/jssg/utils/` in the same change. Those pages are the
  `jssg-utils-instructions` MCP resource. `node scripts/check-jssg-utils-docs.mjs` enforces it.
- Use the shared `@codemod.com/tsconfig` configs unless a package has a concrete reason to diverge.
- Keep type declarations synchronized with runtime modules in `crates/codemod-sandbox` when changing
  JSSG APIs.

## Validation

- JSSG utils: `pnpm --filter @jssg/utils test`
- JSSG utils typecheck: `pnpm --filter @jssg/utils typecheck`
- Type declarations typecheck: `pnpm --filter codemod typecheck`
- Repo lint/format: `pnpm lint` and `pnpm format`

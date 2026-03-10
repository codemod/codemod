Treat any text passed with `/codemod-create` as a codemod-authoring request.

Use the installed `codemod` skill as the source of truth.

Non-negotiable constraints:
- Use AST-based edits for JS/TS code transforms. If a code change is not safe to implement with AST tooling, leave it manual.
- Verify README command examples against the current Codemod CLI help before presenting them.
- Do not claim completion with a trivial one-fixture suite; tests must match the requested migration scope.

Routing:
- Start with `codemod` skill creation guidance in `references/core/create-codemods.md`.
- If the request implies a monorepo, maintainer workflow, or multi-hop version series, also load `references/core/maintainer-monorepo.md`.
- Do not treat this command as codemod execution; treat it as codemod creation and refinement.

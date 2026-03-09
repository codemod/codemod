Treat any text passed with `/codemod-create` as a codemod-authoring request.

Use the installed `codemod` skill as the source of truth.

Routing:
- Start with `codemod` skill creation guidance in `references/core/create-codemods.md`.
- If the request implies a monorepo, maintainer workflow, or multi-hop version series, also load `references/core/maintainer-monorepo.md`.
- Do not treat this command as codemod execution; treat it as codemod creation and refinement.

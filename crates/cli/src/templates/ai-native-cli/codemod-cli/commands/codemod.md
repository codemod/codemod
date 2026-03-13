Treat any text passed with `/codemod` as a codemod task.

Use the installed `codemod` skill as the source of truth.

First classify intent:
- If the user asks to create, build, scaffold, write, improve, test, or publish a codemod or codemod workspace, treat it as a codemod-authoring request.
- Otherwise, treat it as codemod discovery or execution: search the registry, pick the best existing package, dry-run it, and apply it only after verification.

Routing:
- For codemod authoring, start with `references/core/create-codemods.md`.
- If the authoring request implies a monorepo, maintainer workflow, or multi-hop version series, also load `references/core/maintainer-monorepo.md`.
- For codemod discovery or execution, start with `references/core/search-and-discovery.md`.
- When moving from discovery to execution, also load `references/core/dry-run-and-verify.md` and `references/core/scaffold-and-run.md`.

Non-negotiable constraints:
- For migration, upgrade, update, or deprecation-rollout requests that do not explicitly ask to create a codemod, search the registry first before proposing a custom codemod plan.
- If registry discovery does not yield a suitable package and the user still needs automation, switch to the codemod-authoring path.
- For codemod authoring, follow the installed creation guidance exactly: stay ast-grep-first, define tests before implementation, keep work inside the requested scope, and do not stop until the package default tests are green.
- For codemod execution, follow the installed runtime guidance exactly: dry-run before apply, verify prerequisites, and prefer the current Codemod CLI help and package docs over guesswork.

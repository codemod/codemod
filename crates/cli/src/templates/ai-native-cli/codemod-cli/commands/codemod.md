Treat any text passed with `/codemod` as a codemod task.

Use the installed `codemod` skill as the source of truth.

First classify intent:
- If the user asks to create, build, scaffold, write, improve, test, or publish a codemod or codemod workspace, treat it as a codemod-authoring request.
- Otherwise, treat it as codemod discovery or execution: search the registry, pick the best existing package, dry-run it, and apply it only after verification.

Routing:
- For codemod authoring, call `get_codemod_creation_workflow` and `get_jssg_instructions` from Codemod MCP before proceeding.
- If the authoring request implies a monorepo, maintainer workflow, or multi-hop version series, also call `get_codemod_maintainer_monorepo` from Codemod MCP.
- For codemod discovery or execution, call `get_codemod_cli_instructions` from Codemod MCP for command syntax.
- When commands fail or produce unexpected behavior, call `get_codemod_troubleshooting` from Codemod MCP.
- If MCP guidance is temporarily unavailable, continue using the installed skill defaults for package shape, scaffold flags, and search-query quoting instead of blocking.

Non-negotiable constraints:
- For migration, upgrade, update, or deprecation-rollout requests that do not explicitly ask to create a codemod, search the registry first before proposing a custom codemod plan.
- If registry discovery does not yield a suitable package and the user still needs automation, switch to the codemod-authoring path.
- For codemod authoring, follow the creation workflow guidance from Codemod MCP exactly: stay ast-grep-first, define tests before implementation, keep work inside the requested scope, and do not stop until the package default tests are green.
- For codemod execution, follow the CLI instructions from Codemod MCP exactly: dry-run before apply, verify prerequisites, and prefer the current Codemod CLI help and package docs over guesswork.

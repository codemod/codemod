Treat any text passed with `/codemod` as a codemod task.

Use the installed `codemod` skill as the source of truth.

First classify intent:
- If the user asks to create, build, scaffold, write, improve, test, or publish a codemod or codemod workspace, treat it as a codemod-authoring request.
- Otherwise, treat it as codemod discovery or execution: search the registry, pick the best existing package, dry-run it, and apply it only after verification.

Routing:
- If the expected Codemod MCP tools are not actually available in the callable tool list for this session, stop codemod authoring and tell the user to reload/restart Codex and fix Codemod MCP setup first.
- For codemod authoring, call `get_codemod_creation_workflow` first. Before writing source-transform code, call `get_jssg_gotchas` and `get_ast_grep_gotchas`. Call `get_codemod_cli_instructions` only when exact command syntax is needed. Call `get_jssg_instructions` once a package exists and you are implementing the transform.
- If registry search shows no exact package, call `scaffold_codemod_package` immediately.
- Before stopping work on a codemod package, call `validate_codemod_package`.
- If the authoring request uses Node/LLRT APIs, capability-gated modules, or non-trivial multi-file JSSG work, also call `get_jssg_runtime_capabilities`.
- If the authoring request implies a monorepo, maintainer workflow, or multi-hop version series, also call `get_codemod_maintainer_monorepo`.
- For codemod discovery or execution, call `get_codemod_cli_instructions`.
- When commands fail or produce unexpected behavior, call `get_codemod_troubleshooting`.

Non-negotiable constraints:
- For migration, upgrade, update, or deprecation-rollout requests that do not explicitly ask to create a codemod, search the registry first before proposing a custom codemod plan.
- For codemod authoring, inspect only a small representative slice of the repo after or alongside registry discovery, then scaffold and iterate.
- For codemod authoring, stay AST-first, define fixtures before deep implementation, keep work inside the requested scope, and do not stop until workflow validation, package validation, and the package default tests are green.
- For codemod authoring, if symbol origin matters, use semantic analysis and binding-aware checks.
- For codemod authoring, preserve the scaffold-selected package manager in package scripts and package-local README/development commands instead of rewriting them to another runner.
- For codemod authoring/evaluation, do not create commits or push branches unless the user explicitly requested git operations.
- For reusable authored codemods, do not default registry access/visibility to private unless the user explicitly asked for a private package.

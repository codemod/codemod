Treat any text passed with `/codemod` as a codemod task.

Use the installed `codemod` skill as the source of truth.

First classify intent:
- If the user asks to create, build, scaffold, write, improve, test, or publish a codemod or codemod workspace, treat it as a codemod-authoring request.
- Otherwise, treat it as codemod discovery or execution: search the registry, pick the best existing package, dry-run it, and apply it only after verification.

Routing:
- If the expected Codemod MCP tools are not actually available in the callable tool list for this session, stop codemod authoring and tell the user to reload/restart Codex and fix Codemod MCP setup first.
- For codemod authoring, read `codemod-creation-workflow-instructions` first. Before writing source-transform code, read `jssg-gotchas` and `ast-grep-gotchas`. Read `codemod-cli-instructions` only when exact command syntax is needed. Read `jssg-instructions` once a package exists and you are implementing the transform.
- If registry search shows no exact package, run `codemod init` immediately.
- Before stopping work on a codemod package, call `validate_codemod_package`.
- If the authoring request uses Node/LLRT APIs, capability-gated modules, or non-trivial multi-file JSSG work, also read `jssg-runtime-capabilities-instructions`.
- If the authoring request implies a monorepo, maintainer workflow, or multi-hop version series, also read `codemod-maintainer-monorepo-instructions`.
- For codemod discovery or execution, read `codemod-cli-instructions`.
- When commands fail or produce unexpected behavior, read `codemod-troubleshooting-instructions`.

Non-negotiable constraints:
- For migration, upgrade, update, or deprecation-rollout requests that do not explicitly ask to create a codemod, search the registry first before proposing a custom codemod plan.
- For codemod authoring, inspect only a small representative slice of the repo after or alongside registry discovery, then scaffold and iterate.
- For codemod authoring, stay AST-first, define fixtures before deep implementation, keep work inside the requested scope, and do not stop until workflow validation, package validation, and the package default tests are green.
- For codemod authoring, if symbol origin matters, use semantic analysis and binding-aware checks.
- For codemod authoring, before stopping, re-check the whole package surface: `README.md`, `codemod.yaml`, `workflow.yaml`, `package.json` scripts, tests/fixtures, and any renamed paths or ids. Update all affected files together instead of leaving stale metadata or docs behind.
- For codemod authoring, preserve the scaffold-selected package manager in package scripts and package-local README/development commands instead of rewriting them to another runner.
- For codemod authoring/evaluation, do not create commits or push branches unless the user explicitly requested git operations.
- For reusable authored codemods, do not default registry access/visibility to private unless the user explicitly asked for a private package.

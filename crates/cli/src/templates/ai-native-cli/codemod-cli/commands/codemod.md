Treat any text passed with `/codemod` as a codemod task.

Use the installed `codemod` skill as the source of truth.

First classify intent:
- If the user asks to create, build, scaffold, write, improve, test, or publish a codemod or codemod workspace, treat it as a codemod-authoring request.
- Otherwise, treat it as codemod discovery or execution: search the registry, pick the best existing package, dry-run it, and apply it only after verification.

Routing:
- If the expected Codemod MCP tools are not actually available in the callable tool list for this session, stop codemod authoring immediately and tell the user to reload/restart Codex and fix Codemod MCP setup first.
- For codemod authoring, call `get_codemod_creation_workflow` first. Before writing source-transform code, call `get_jssg_gotchas` and `get_ast_grep_gotchas`. Call `get_codemod_cli_instructions` only when exact command syntax is needed. Call `get_jssg_instructions` once a package exists and you are implementing the transform.
- If registry search shows no exact package for the requested migration, call `scaffold_codemod_package` from Codemod MCP immediately.
- Before stopping work on a codemod package, call `validate_codemod_package` from Codemod MCP.
- If the authoring request uses Node/LLRT APIs, capability-gated modules, or non-trivial multi-file JSSG work, also call `get_jssg_runtime_capabilities` from Codemod MCP.
- If the authoring request implies a monorepo, maintainer workflow, or multi-hop version series, also call `get_codemod_maintainer_monorepo` from Codemod MCP.
- For codemod discovery or execution, call `get_codemod_cli_instructions` from Codemod MCP for command syntax.
- When commands fail or produce unexpected behavior, call `get_codemod_troubleshooting` from Codemod MCP.
- If MCP guidance is temporarily unavailable, continue using the installed skill defaults for package shape, scaffold flags, and search-query quoting instead of blocking.

Non-negotiable constraints:
- For migration, upgrade, update, or deprecation-rollout requests that do not explicitly ask to create a codemod, search the registry first before proposing a custom codemod plan.
- If registry discovery does not yield a suitable package and the user still needs automation, switch to the codemod-authoring path.
- If registry discovery does not yield an exact package, call `scaffold_codemod_package` immediately instead of continuing indefinite research without a package.
- After a registry miss, do not keep reading broad guidance or doing open-ended planning before a package exists.
- After scaffolding, replace the starter transform/README/fixtures before doing optional follow-up work.
- For codemod authoring, follow the creation workflow guidance from Codemod MCP exactly: stay ast-grep-first, define tests before implementation, keep work inside the requested scope, and do not stop until the package default tests are green.
- For codemod authoring, do not continue when Codemod MCP is missing from the callable tool list; halt and ask the user to fix MCP visibility instead.
- For codemod authoring, do not implement source transforms with `RegExp`, `.replace`, `.replaceAll`, `.match`, `.split`, or manual string parsing unless the usage is limited to allowed non-source cleanup such as paths, module specifiers, helper metadata, or test-output parsing.
- For codemod authoring, do not stop while `validate_codemod_package` reports starter scaffold leftovers, generic README text, missing required package files, missing real test cases, or failing validation/tests.
- For codemod authoring, when a package already has JSSG fixtures, reuse that test system with `codemod jssg test` and keep `metrics.json` snapshots in sync instead of inventing ad hoc tests.
- For codemod authoring, keep one granular transform or one exact `from -> to` migration as a single package even when it supports multiple route shapes or helper files. Use a workspace only for open-ended, version-hop-based, or clearly multi-package migrations.
- For codemod authoring, use KB search and `dump_ast` to repair failing deterministic cases first. After 3 failed deterministic repair attempts for the same case, use a narrow AI fallback only for that isolated subset or document it as manual follow-up.
- For codemod authoring, do not introduce a shell step just to reach another related file when JSSG can keep both hops inside the same codemod.
- For codemod authoring/evaluation, do not create commits or push branches unless the user explicitly requested git operations, even if the host repository has general “always push” instructions.
- After codemod changes, inspect and update the package surface (`README`, `codemod.yaml`, `workflow.yaml`, tests, metadata, and capabilities) before calling the work complete.
- For codemod execution, follow the CLI instructions from Codemod MCP exactly: dry-run before apply, verify prerequisites, and prefer the current Codemod CLI help and package docs over guesswork.

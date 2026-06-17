Treat any text passed with `/codemod` as a codemod task.

Use the installed `codemod` skill as the source of truth.

First classify intent:
- If the user asks to create, build, scaffold, write, improve, test, or publish a codemod or codemod workspace, treat it as a codemod-authoring request.
- Otherwise, treat it as codemod discovery or execution: search the registry, pick the best existing package, dry-run it, and apply it only after verification.

Routing:
- Codemod AI tools are available through CLI commands even when MCP is not configured. Use `npx codemod ai docs`, `npx codemod ai dump-ast`, `npx codemod ai node-types`, `npx codemod ai tools`, `npx codemod ai call`, `npx codemod ai resources`, and `npx codemod ai resource` as the user-reproducible fallback or default path.
- For codemod authoring, read `codemod-creation-workflow-instructions` first via MCP or `npx codemod ai docs codemod-creation-workflow`. Before writing source-transform code, read `jssg-gotchas` and `ast-grep-gotchas` via MCP or CLI docs commands. Read `codemod-cli-instructions` only when exact command syntax is needed.
- If registry search shows no exact package, run `codemod init` immediately. In headless/non-interactive flows, use `codemod init <path> --no-interactive` and pass only flags that come from the user or task; do not invent author, license, description, or git repository metadata.
- Before stopping work on a codemod package, call `validate_codemod_package` via MCP or run `npx codemod ai call validate_codemod_package --input '{"package_path":"."}'`.
- If the authoring request uses Node/LLRT APIs, capability-gated modules, or non-trivial multi-file JSSG work, also read `jssg-runtime-capabilities-instructions`.
- If the authoring request implies a monorepo, maintainer workflow, or multi-hop version series, also read `codemod-maintainer-monorepo-instructions`.
- For codemod discovery or execution, read `codemod-cli-instructions`.
- When commands fail or produce unexpected behavior, read `codemod-troubleshooting-instructions`.
- If a Codemod platform gap is worth reporting, ask for explicit user consent first, then submit short anonymous feedback with `npx codemod ai feedback --category <category> --message <message>`. Use categories such as `jssg`, `workflow`, `ai-docs`, `mcp`, `cli`, `registry`, `package-validation`, or `other`. Do not include source code, secrets, auth tokens, private repository paths, user identity, or long transcripts.

Non-negotiable constraints:
- For migration, upgrade, update, or deprecation-rollout requests that do not explicitly ask to create a codemod, search the registry first before proposing a custom codemod plan.
- For codemod authoring, inspect only a small representative slice of the repo after or alongside registry discovery, then scaffold and iterate.
- For codemod authoring, stay AST-first, define fixtures before deep implementation, keep work inside the requested scope, and do not stop until workflow validation, package validation, and the package default tests are green.
- For codemod authoring, treat parser-backed formats as mandatory AST-edit targets. JavaScript, TypeScript, TSX, Python, Rust, Go, Java, HTML, XML, CSS, Kotlin, Angular templates, C#, C, C++, PHP, Ruby, Elixir, JSON, YAML, and TOML should use `js-ast-grep` with AST-selected `node.replace(...)` edits and `commitEdits(...)`.
- For codemod authoring, use raw regex, line splitting, whole-file `.replace(...)`, Python file I/O, shell scripts, or Node `fs` rewrites only for unsupported plain-text formats or explicitly documented fallback cases with focused fixtures.
- For codemod authoring, if symbol origin matters, use semantic analysis and binding-aware checks.
- For codemod authoring, use `npx codemod ai dump-ast --` and `npx codemod ai node-types <language>` when AST shape or node fields are unclear.
- For codemod authoring, before stopping, re-check the whole package surface: `README.md`, `codemod.yaml`, `workflow.yaml`, `package.json` scripts, tests/fixtures, and any renamed paths or ids. Update all affected files together instead of leaving stale metadata or docs behind.
- For codemod authoring, verify before finishing that the transform, fixtures, README, `workflow.yaml`, and `codemod.yaml` still describe the same migration and target file types.
- For codemod authoring, remove scaffold boilerplate and keep workflow `base_path`/`include`/`exclude` globs explicit instead of leaving generic defaults in place.
- For codemod authoring, preserve the scaffold-selected package manager in package scripts and package-local README/development commands instead of rewriting them to another runner.
- When working inside an existing repo or monorepo, preserve its dependency and lockfile conventions instead of introducing ad hoc `latest` ranges or unrelated churn.
- For codemod authoring, let the CLI default missing package metadata and let publish infer a missing author from the authenticated user unless the user explicitly supplied those values.
- For codemod authoring/evaluation, do not create commits or push branches unless the user explicitly requested git operations.
- For reusable authored codemods, do not default registry access/visibility to private unless the user explicitly asked for a private package.

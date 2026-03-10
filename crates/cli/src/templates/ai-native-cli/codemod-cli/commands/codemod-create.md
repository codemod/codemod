Treat any text passed with `/codemod-create` as a codemod-authoring request.

Use the installed `codemod` skill as the source of truth.

Non-negotiable constraints:
- Use AST-based edits for JS/TS code transforms. If a code change is not safe to implement with AST tooling, leave it manual.
- Default to ast-grep-based codemods when creating codemod packages: use `js-ast-grep` for JS/TS-family source changes and `ast-grep` workflow steps for other deterministic structured edits when possible.
- Multi-step workflows are acceptable and preferred when the migration spans multiple safe transformation surfaces. Do not use shell/native scripts as the primary transformation engine unless the user explicitly asked for that implementation style or no ast-grep-based path is viable.
- Treat dependency/version manifest upgrades as part of the core migration surface when the researched upgrade path requires them. If official docs require package, SDK, plugin, or toolchain version bumps and those edits are deterministic, automate them instead of leaving them implicit or optional.
- Do not default to analysis-only codemods when the user asked for an actual migration codemod. Use analysis-only output only when the researched migration has no safe, meaningful automatable source edits or when the user explicitly asked for analysis/reporting.
- Verify README command examples against the current Codemod CLI help before presenting them.
- Do not claim completion with a trivial one-fixture suite; tests must match the requested migration scope.
- Default to workflow-only codemods. Do not scaffold package agent skills or use `--with-skill` / `--skill` unless the user explicitly asks for agent-skill behavior.
- Stay within the requested migration scope. You may suggest adjacent or optional migrations, but do not scaffold extra packages for them unless the user explicitly asked for that expansion.
- Define the test matrix and create the initial fixtures before implementing transforms. Do not implement codemods first and invent tests afterward.
- When debugging or validating JSSG tests, run the direct command `codemod jssg test -l <language> ./scripts/codemod.ts -v --strictness loose` and add `--filter <case>` when isolating failures. Do not rely on `npm test` for debugging because it does not guarantee verbose failure output.
- Do not stop while the package's normal/default test command is red. Metrics snapshot mismatches are still test failures and must be fixed before summarizing the work.
- If loose-mode testing is used during iteration, update fixtures at the end and confirm the package passes its normal/default test command before summarizing the work.
- For non-trivial codemod creation, use Codemod MCP as part of the active authoring loop: use it during planning, AST/pattern refinement, and test/debug iteration. If MCP is unavailable, fall back to the installed `codemod` skill references and current Codemod CLI help.

Routing:
- Start with `codemod` skill creation guidance in `references/core/create-codemods.md`.
- If the request implies a monorepo, maintainer workflow, or multi-hop version series, also load `references/core/maintainer-monorepo.md`.
- Do not treat this command as codemod execution; treat it as codemod creation and refinement.

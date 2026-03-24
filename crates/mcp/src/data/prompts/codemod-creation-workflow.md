# Codemod Creation Workflow

Use this guide when the task is to create or improve codemods, not just run them.

## Default workflow

1. Plan the migration before touching files.
2. Search the target codebase for representative "before" examples before choosing package shape.
3. Research the migration path on the web before choosing package shape.
4. Scaffold with `codemod init`.
5. Define the test matrix from the discovered repo patterns and migration docs before implementing transforms.
6. Create the initial test fixtures before implementing transforms.
7. Use Codemod MCP while authoring the codemod against those tests.
8. Run the codemod test suite, fix failures, and iterate on tricky cases before publishing.

## Hard rules

- Use AST-based edits for code transforms. Do not implement codemods as raw source-string replacement or regex replacement over the full file text.
- Do not use broad identifier sweeps that rename every matching token by text alone. Match the intended API context explicitly, for example import specifiers, namespaced/member expressions, relevant object properties, or specific constructor/call-expression sites.
- Default to `js-ast-grep`-based codemod packages when creating codemods.
- Use multi-step workflows when the migration spans multiple safe sequential or independent transformation surfaces, for example JS/TS source plus JSON manifests plus Gradle or Podfile edits.
  - If the files are related, use `jssgTransform`. For example a codemod that changes React components along with imported CSS modules, should use `jssgTransform(...)` function to modify the adjacent CSS module.
- Do not switch to shell/native scripts as the primary transformation engine unless the user explicitly asked for that implementation style or no ast-grep-based path is viable.
- Treat dependency/version manifest upgrades as part of the core migration surface when the researched upgrade path requires them. If official docs call for package, SDK, plugin, or toolchain version bumps and the edits are deterministic, automate them in the workflow.
- If the researched migration includes native/build/config work, automate the safe deterministic edits with `ast-grep` workflow steps when possible, including manifest/version bumps, and leave only the unsafe or environment-specific remainder as manual follow-up.
- Do not fall back to an analysis-only codemod if the user asked for an actual migration codemod and there are safe, meaningful automatable source edits available.
- An analysis-only codemod is acceptable only when research shows there are no safe, meaningful automatable source edits for the requested migration, or when the user explicitly asked for analysis/reporting output.
- If a code change cannot be encoded safely with AST tooling, document it as a manual step instead of shipping a brittle transform.
- A manual-only hop is acceptable only when the research shows there is no safe, meaningful automatable source change for that hop.
- Tests must be comprehensive relative to the user's request, not just the easiest documented example.

## Use sub-agents when the task is large

- Small, exact, one-hop codemods can stay in one agent.
- For non-trivial work, split the job into focused sub-agents:
  - research agent for web/docs gathering,
  - codebase analysis agent for "before" examples and edge-case discovery,
  - test agent for test-matrix definition and initial fixture generation,
  - implementation agent for the transform against the defined fixtures.
- For multi-hop upgrades, use one coordinator plus per-hop research and implementation/test agents when parallel work is possible.
- Keep the coordinator responsible for the final package shape, execution order, and summary.

## Decide between single package and workspace

- Default to a single codemod when the user gives an exact source and target version.
- Default to a workspace when the user asks to "upgrade to latest", "stay up to date", or otherwise leaves the version range open-ended.
- Use the codebase examples and web research together when making this decision; do not decide from docs alone if the target repo shows materially different patterns.
- Before deciding, inspect official migration docs, changelogs, or upgrade guides and determine whether the migration is documented as sequential version hops.
- If the docs show separate upgrade guides for intermediate versions, create a workspace and generate one codemod per documented hop.
- If the docs show one direct path with no intermediate hops, keep a single package unless the user explicitly wants a monorepo.
- Keep the generated packages inside the requested migration scope. If research reveals adjacent or optional migrations, list them as follow-up recommendations instead of scaffolding them automatically.
- Only add an adjacent migration package when the user explicitly asked for it or clearly approved expanding scope beyond the original request.

## Search the target codebase first

1. Inspect the current repo for real "before" examples that the codemod must transform.
2. Cluster those examples into concrete transformation patterns.
3. Record edge cases, no-op cases, and patterns that should be left unchanged.
4. Use those findings to refine the migration plan before implementation starts.

The agent should not rely only on abstract migration docs when the actual codebase reveals usage variants that affect transform behavior.

## Required research flow

1. Search the web first for migration guidance. Prefer the package's official migration guide, release notes, or upgrade docs, but also collect other credible sources when they add missing context, examples, edge cases, or ecosystem-specific gotchas.
2. Build a version-hop plan before scaffolding anything.
3. Record the supported hop order, breaking changes per hop, and any steps that are manual-only.
4. Only after the hop plan is stable, choose `codemod init` shape and start implementation.

When researching:

- Treat official docs as the primary source of truth when they exist.
- Add high-signal secondary sources when they materially improve the plan, for example framework migration blog posts, maintainer release notes, package changelogs, GitHub issues, or well-maintained upgrade guides.
- Cross-check secondary sources against official docs before encoding behavior in a codemod.
- If official docs are missing or incomplete, state that explicitly and base the plan on the best available sources instead of skipping web research.
- If you decide a hop is manual-only, record why it is manual-only and which researched changes were deemed unsafe or non-automatable.

When the migration has multiple independent hop guides:

- Gather those guides in parallel when possible.
- Gather supporting secondary sources in parallel when they help explain edge cases for specific hops.
- Plan each hop separately.
- Keep the final execution order explicit in the workspace README and package descriptions.

Example: if official docs expose separate guides such as `before-v5`, `v5-to-v6`, `v6-to-v7`, and `v7-to-v8`, treat that as a workspace migration series rather than a single codemod.

## Scaffold

- Default to workflow-only ast-grep-based codemods and workspaces. Do not add package skill assets unless the user explicitly asked for agent-skill behavior.
- Interactive:
  - `codemod init`
- Non-interactive jssg:
  - `codemod init my-codemod --project-type ast-grep-js --language typescript --package-manager npm --description "Example codemod" --author "Your Name" --license MIT --no-interactive`
- Monorepo workspace:
  - `codemod init my-codemod-repo --workspace --project-type ast-grep-js --language typescript --package-manager npm --description "Example codemod" --author "Your Name" --license MIT --no-interactive`

## Multi-hop workspace execution

- For upgrade series, scaffold a workspace first, then add one codemod per hop under `codemods/<slug>/`.
- Name packages so the hop is obvious, for example `react-native-sentry-v5-to-v6`.
- If the user asked for an evergreen or "latest" migration, the workspace should describe the full recommended hop chain from the oldest supported entrypoint to the newest supported target.
- If one hop is manual-only, still keep it documented in the workspace so the execution order remains complete.

## Codemod MCP guidance

- For non-trivial codemod creation, use Codemod MCP throughout the authoring loop, not just once at startup.
- Planning stage:
  - Call `get_jssg_instructions` before writing non-trivial jssg transforms.
  - Call `get_codemod_cli_instructions` before finalizing project/workflow setup and README command examples.
  - Call `get_jssg_utils_instructions` when imports or import helpers are part of the codemod work.
- AST/pattern refinement stage:
  - Use `dump_ast` while narrowing patterns, validating AST shapes, and confirming that a transform is matching the intended code context.
- Test/debug stage:
  - Use Codemod MCP again while iterating on failing tests or unclear transform behavior instead of treating MCP as a one-time bootstrap.
  - When available for the current task, prefer MCP-assisted test execution or AST inspection before falling back to ad hoc shell debugging.
- When migration patterns depend on symbol origin or cross-file references, use semantic analysis.
- Enable `semantic_analysis: workspace` in the workflow when symbol definition or reference checks matter.
- If Codemod MCP is unavailable, fall back to the installed `codemod` skill references and current Codemod CLI help instead of inventing workflow or command behavior.

## Expected package shape

- Every codemod package should have `workflow.yaml` and `codemod.yaml`.
- The default codemod package shape is ast-grep-based, typically `js-ast-grep` with `scripts/codemod.ts` for JS/TS-family source edits and additional `ast-grep` workflow steps when the migration also includes other deterministic structured files.
- Do not replace that default package shape with shell scripts unless the user explicitly asked for a shell/native workflow or there is no viable ast-grep-based path.
- In monorepos, each codemod should live under `codemods/<slug>/`.

## Validate and test

- Validate workflow/package structure:
  - `codemod workflow validate -w codemods/<slug>/workflow.yaml`
- Define the test matrix before implementing transforms, using the repo-derived examples and migration-doc cases you already collected.
- Create the initial fixtures before writing the transform so implementation is driven by expected behavior rather than post-hoc patching.
- During implementation and debugging, run the direct JSSG test command rather than `npm test`, so you control verbosity, strictness, and filtering and get actionable failure output:
  - `codemod jssg test -l <language> ./scripts/codemod.ts -v --strictness loose`
- Use `--filter <case>` when isolating failures.
- Prefer `--strictness loose` for codemod verification so formatting-only differences do not hide semantic progress. Only tighten strictness when exact formatting is part of the requested outcome.
- Create snapshot-style test fixtures that cover the actual discovered scope, following the documented JSSG test layout:
  - `tests/<case>/input.*`
  - `tests/<case>/expected.*`
- Include:
  - representative repo-derived cases,
  - realistic cases from migration docs or release notes when the repo does not expose enough examples,
  - edge cases,
  - preserve/no-op cases,
  - negative cases where similar code should stay unchanged,
  - any version-hop-specific cases that change behavior.
- The fixture set should cover the migration scope the user asked for. If the request is broad, the test matrix must be broad as well.
- As a minimum, each non-trivial hop should include:
  - one realistic/doc-derived case,
  - one edge case,
  - one preserve/no-op case,
  - one negative case,
  - and one repo-derived case when the target repo exposes relevant examples.
- Run the codemod test suite after implementation and fix the codemod or the test fixture set until the suite passes.
- Use `--strictness loose` while iterating when formatting noise would otherwise hide semantic mismatches, but do not stop there.
- As the final verification step, update snapshots only after the transform behavior is settled, then rerun the package's normal/default test command, for example `npm test`, so the checked-in fixtures pass for users without requiring loose-mode flags.
- Treat metrics snapshots the same as code snapshots: if `metrics.json` mismatches, refresh or fix the expected metrics and rerun the default package tests before summarizing.
- For local verification against a repo:
  - `codemod workflow run -w codemods/<slug>/workflow.yaml --target <repo-path>`

## Test loop expectations

- Do not implement the codemod blindly and generate tests afterward; define the expected cases first and implement against them.
- After writing or updating the codemod, run the test suite before presenting the result.
- If tests fail, rerun the failing case with `codemod jssg test -l <language> ./scripts/codemod.ts -v --strictness loose --filter <case>`, inspect the expected-vs-actual diff, and fix the codemod rather than leaving the failure unexplained.
- If the migration scope is broad, keep expanding the tests until the discovered codebase patterns are covered.
- For multi-hop workspaces, run tests per package and keep each hop green independently.
- Do not present a codemod as complete if the tests only prove a trivial happy path.
- Do not present a codemod as complete while any package default test command is failing, including failures caused only by snapshot or metrics ordering mismatches.
- If you used loose-mode during debugging, the final step before summarizing is to refresh the expected fixtures as needed and rerun the default test command without extra debugging flags.
- Before documenting local run or validation commands in a README, verify them against `codemod --help`, `codemod workflow --help`, or the relevant subcommand help.

## Publish expectations

- Keep codemods on the current branch unless the user explicitly wants branch automation.
- Do not push automatically.
- Use trusted publisher/OIDC based publishing when wiring GitHub Actions.
- For multi-hop workspaces, validate every hop independently before proposing publish automation for the full series.

---
name: codemod
description: Use Codemod CLI whenever the user wants to migrate, upgrade, update, or refactor a codebase in a repeatable way. This includes framework migrations, library upgrades, version bump migrations, API surface changes, deprecations, large-scale mechanical edits, codemod package authoring, AST inspection, tree-sitter node type lookup, and Codemod documentation lookup through `npx codemod ai` commands. First search the Codemod Registry for an existing package, prefer deterministic codemods before open-ended AI rewrites, run dry-runs before apply, and create a codemod package only when no suitable package exists. For authoring and publishing SDK/library version-migration packages from a changelog or migration guide, use the sibling sdk-migration skill.
allowed-tools:
  - Bash(codemod *)
  - Bash(npx codemod *)
argument-hint: "<migration-intent>"
---

# Codemod Migration Assistant

codemod-compatibility: mcs-v1
codemod-skill-version: 1.2.1

Use this skill for migrations, upgrades, deprecation rollouts, large mechanical refactors, codemod package authoring (including read-only mining / Insights metrics packages), and AST inspection. Use Codemod before falling back to an open-ended AI rewrite.

Everything below runs through the Codemod CLI. For work on the user's Codemod platform account (Grep across their repositories, Insights dashboards, campaigns, organizations), use the Codemod MCP at `https://app.codemod.com/api/v1/mcp`. If its tools are not available, ask the user to connect it (setup: https://docs.codemod.com/enterprise/codemod-mcp); do not substitute these CLI commands for platform data.

## CLI commands

- Search the registry: `npx codemod search <terms>`
- Dry-run a package: `npx codemod run <package-id> --dry-run`
- Scaffold a package: `npx codemod init <path> --no-interactive`
- List docs: `npx codemod ai docs`
- Read a doc: `npx codemod ai docs <name>`, for example `npx codemod ai docs codemod-creation-workflow`
- Dump AST: `npx codemod ai dump-ast src/App.tsx` or `echo 'const x = 1;' | npx codemod ai dump-ast --`
- Node kinds and fields: `npx codemod ai node-types tsx`
- Validate a package: `npx codemod ai call validate_codemod_package --input '{"package_path":"."}'`
- Report a platform gap, only after the user consents: `npx codemod ai feedback --category <category> --message "<short message>"`

## Which doc to read

| When you | Run |
| --- | --- |
| Create a codemod or do a large refactor (read first) | `npx codemod ai docs codemod-creation-workflow` |
| Write transform or mining code (read before coding) | `npx codemod ai docs jssg-gotchas`, then `npx codemod ai docs ast-grep-gotchas` |
| Implement the transform once a package exists | `npx codemod ai docs jssg` |
| Need Node/LLRT APIs, capability-gated modules, or multi-file JSSG work | `npx codemod ai docs jssg-runtime-capabilities` |
| Need reusable JSSG helpers | `npx codemod ai docs jssg-utils` |
| Run or discover codemods, or need exact command syntax | `npx codemod ai docs codemod-cli` |
| Maintain a codemod monorepo or a multi-hop version series | `npx codemod ai docs codemod-maintainer-monorepo` |
| Split a large migration into multiple PRs | `npx codemod ai docs sharding` |
| Hit errors or unexpected behavior | `npx codemod ai docs codemod-troubleshooting` |

The Performant codemods section of `jssg-gotchas` is required reading for any JSSG package: discriminating selectors or token gates, no per-file full-repo walks, and a timed no-op vs real run on the same entry path (`workflow run` when the package uses selectors/globs; otherwise `jssg run --language`) before you claim it is done.

For mining / Insights metrics packages, Insights loads the `js-ast-grep` script of the one bound workflow step: call `useMetricAtom` and `increment` in that script and `return null`, not only in a package-entry file or helper import.

## SDK version migration packages

When the user wants to **author and publish** codemods for an SDK or library upgrade and provides a source version, target version, and a changelog / migration guide / helpful link: use the sibling `sdk-migration` skill at `sdk-migration/SKILL.md` (same skills root as this file). If it is not installed, fetch and follow `https://raw.githubusercontent.com/codemod/codemod/main/skills/sdk-migration/SKILL.md` before scaffolding.

That workflow's outputs are: a public-by-default migration plan (with author name/email from `npx codemod whoami --detailed` or the user), locally built packages, tested packages with fix-and-retest until green, then `npx codemod publish`.

## Migrations: search the registry first

1. Before planning a manual migration, derive a few high-signal terms from the request and run `npx codemod search`. Skip this only when the user says not to use Codemod or not to search.
2. If a package fits, read its README for prerequisites, config, and limits, then run it with `--dry-run` before applying. Use `--no-interactive --install-skill` when the package ships an installable skill.
3. Verify with tests and the dry-run summary before applying.
4. If no package fits, run `npx codemod init` right away instead of continuing broad research.

## Authoring rules

- Keep transforms AST-first. Parser-backed formats (JavaScript, TypeScript, TSX, Python, Rust, Go, Java, HTML, XML, CSS, Kotlin, Angular templates, C#, C, C++, PHP, Ruby, Elixir, JSON, YAML, TOML) use a `js-ast-grep` step: AST queries select the nodes, `node.replace(...)` edits them, `root.root().commitEdits(...)` applies the edits.
- Do not use regex, line splitting, whole-file `.replace(...)`, or Python, shell, or Node `fs` rewrites for those formats, and do not use JSSG only as a file walker around such rewrites. Do not add a shell step to reach a related file when JSSG can make that hop. Use raw text only for unsupported plain-text formats, document the fallback, and cover it with fixtures.
- Run `npx codemod ai dump-ast` before broadening heuristics and `npx codemod ai node-types` when node kinds or fields are unclear. Use semantic, binding-aware checks when symbol origin matters.
- Keep one granular transform or one exact `from -> to` migration per package unless the request is clearly open-ended or multi-hop.
- Inspect only 1-3 representative repo files before you settle the transform shape.
- In non-interactive `codemod init`, pass only flags the user or task gave you. Do not invent `--author`, `--license`, `--description`, or `--git-repository-url`; leave missing metadata to CLI defaults and the publish-time auth fallback.
- Define positive, negative, edge, and no-op fixtures before deep implementation, then replace the starter transform, README, and fixtures.
- Before stopping, make the transform, fixtures, README, `codemod.yaml`, `workflow.yaml`, and `package.json` scripts describe the same migration and target files. Keep workflow `base_path`/`include`/`exclude` globs explicit and aligned with `codemod.yaml` `targets.languages`. Do not churn version numbers, but do not leave stale metadata after a rename.
- Keep the scaffold's package manager in scripts and README commands, and the repository's dependency and lockfile conventions.
- Do not stop while `validate_codemod_package` reports starter scaffold markers, missing package surface updates, missing real test cases, or failing default tests.
- Do not default reusable codemods to private registry access unless the user asked for it.
- Do not commit or push unless the user asked for git operations.

## Feedback

When you find a Codemod platform gap, ask the user for consent before running `npx codemod ai feedback`. Use a category such as `jssg`, `workflow`, `ai-docs`, `mcp`, `cli`, `registry`, `package-validation`, or `other`. Keep the message short and thematic, with no source code, secrets, tokens, private repository paths, user identity, or transcripts.

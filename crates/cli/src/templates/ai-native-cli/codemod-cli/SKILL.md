---
name: codemod
description: Use Codemod CLI whenever the user wants to migrate, upgrade, update, or refactor a codebase in a repeatable way. This includes framework migrations, library upgrades, version bump migrations, API surface changes, deprecations, large-scale mechanical edits, codemod package authoring, AST inspection, tree-sitter node type lookup, and Codemod documentation lookup through `codemod ai` commands. First search the Codemod Registry for an existing package, prefer deterministic codemods before open-ended AI rewrites, run dry-runs before apply, and create a codemod package only when no suitable package exists.
allowed-tools:
  - Bash(codemod *)
  - Bash(npx codemod *)
argument-hint: "<migration-intent>"
---

# Codemod Migration Assistant

codemod-compatibility: mcs-v1
codemod-skill-version: 1.1.0

Use this skill to orchestrate migration execution, codemod package authoring, and Codemod CLI AI inspection.

Trigger this skill when the user asks to:
- migrate from one framework/library/tooling stack to another
- upgrade or update a framework, SDK, package, plugin, compiler, or toolchain
- apply a breaking-change migration, deprecation migration, or version bump rollout
- perform a large mechanical refactor that may already exist as a Codemod Registry package
- inspect AST shape, tree-sitter node types, Codemod documentation, or Codemod MCP-equivalent tools/resources from the CLI

When the intent is migration/update/upgrade oriented, use Codemod first before defaulting to a fully open-ended AI rewrite.

## CLI AI tools

Codemod AI tools must be usable without MCP. Prefer the CLI commands below when MCP tools/resources are missing, stale, blocked by the harness, or when you need a user-reproducible command:

- Dump AST shape: `echo 'const x = 23;' | npx codemod ai dump-ast --`
- Dump AST for a file: `npx codemod ai dump-ast src/App.tsx`
- Inspect node types for a language: `npx codemod ai node-types tsx`
- List docs resources: `npx codemod ai docs`
- Read/search docs: `npx codemod ai docs codemod-creation-workflow`
- Submit anonymous platform feedback after user consent: `npx codemod ai feedback --category jssg --message "Short feedback"`

If MCP is available, direct MCP calls are acceptable. If MCP is unavailable, do not stop authoring only because MCP is missing; use the `codemod ai` CLI equivalents.

When the user:
- **Creates a codemod or does a large refactor** — Read `codemod-creation-workflow-instructions` first via MCP or `npx codemod ai docs codemod-creation-workflow`. Before writing source-transform code, read `jssg-gotchas` and `ast-grep-gotchas` via MCP or `npx codemod ai docs jssg-gotchas` / `npx codemod ai docs ast-grep-gotchas`. Read `codemod-cli-instructions` only when you need exact command syntax. Read `jssg-instructions` once a package exists and you are implementing the transform.
- **Needs to know whether a codemod package is still a starter scaffold or incomplete** — Call MCP `validate_codemod_package` or run `npx codemod ai call validate_codemod_package --input '{"package_path":"."}'` before stopping.
- **Needs Node/LLRT APIs, capability-gated modules, or non-trivial multi-file JSSG work** — Read `jssg-runtime-capabilities-instructions` via MCP or `npx codemod ai docs jssg-runtime-capabilities`.
- **Maintains a codemod monorepo** — Read `codemod-maintainer-monorepo-instructions` via MCP or `npx codemod ai docs codemod-maintainer-monorepo`.
- **Runs or discovers codemods** — Read `codemod-cli-instructions` via MCP or `npx codemod ai docs codemod-cli`.
- **Hits errors or unexpected behavior** — Read `codemod-troubleshooting-instructions` via MCP or `npx codemod ai docs codemod-troubleshooting`.
- **Needs reusable JSSG helpers** — Read `jssg-utils-instructions` via MCP or `npx codemod ai docs jssg-utils`.
- **Needs to split a large migration into multiple PRs** — Read `sharding-instructions` via MCP or `npx codemod ai docs sharding`.

## Anonymous feedback

If you discover a Codemod platform gap, ask the user for explicit consent before enabling or submitting anonymous feedback. With consent, run `npx codemod ai feedback --category <category> --message <message>`. The command records the consent timestamp in user-wide Codemod config and submits the short feedback anonymously.

Use these categories when possible: `jssg` for JSSG feature requests or runtime gaps, `workflow` for workflow authoring feedback, `ai-docs` for docs/resource enhancements, `mcp` for MCP tool or resource feedback, `cli`, `registry`, `package-validation`, or `other`.

Keep feedback short and thematic. Do not include source code, secrets, auth tokens, private repository paths, user identity, or long transcripts.

## Authoring defaults

- Treat the Codemod docs served through `codemod ai docs` or MCP resources as the source of truth for CLI, workflow, and JSSG semantics.
- Keep source transforms AST-first. Do not use regex or raw source-text rewriting as the primary implementation strategy.
- Before using line-based parsing, regular-expression replacement, or whole-file source rewriting, check whether the target file type has a JSSG language parser. Parser-backed formats include JavaScript, TypeScript, TSX, Python, Rust, Go, Java, HTML, XML, CSS, Kotlin, Angular templates, C#, C, C++, PHP, Ruby, Elixir, JSON, YAML, and TOML.
- For parser-backed files, use a `js-ast-grep` workflow with the matching language import, AST queries (`find`, `findAll`, `getMatch`, fields), `node.replace(...)`, and `root.root().commitEdits(...)`. String construction is acceptable for the replacement text, but AST-selected nodes must define the edit boundaries.
- Use raw text manipulation only for unsupported plain-text formats or when a structured parser is unavailable. When you do, document the fallback in code or README and cover comments, multiline values, quoting, false positives, and no-op cases in fixtures.
- Use `npx codemod ai dump-ast` or MCP `dump_ast` before broadening heuristics.
- Use `npx codemod ai node-types <language>` or MCP `get_node_types` when node kinds or fields are unclear.
- If symbol origin matters, use semantic analysis and binding-aware checks.
- Keep one granular transform or one exact `from -> to` migration as a single package unless the request is clearly open-ended or multi-hop.
- Inspect 1-3 representative repo files after or alongside registry discovery before you finalize the transform shape.
- If registry search yields no exact package, run `codemod init` immediately instead of continuing broad research without a package. In headless/non-interactive flows, use the simplified `codemod init <path> --no-interactive` interface and pass only user- or task-provided flags; do not invent `--author`, `--license`, `--description`, or `--git-repository-url`.
- After the package exists, replace the starter transform, README, and starter fixtures before doing optional work.
- Define positive, negative, and edge fixtures before deep implementation work.
- Before stopping, inspect the whole package surface and update every affected file together: `README.md`, `codemod.yaml`, `workflow.yaml`, `package.json` scripts, tests/fixtures, and any renamed paths, ids, or references. Do not churn version numbers by default, but do not leave stale package metadata behind after a rename or material package-surface change.
- Keep the requested migration aligned across every artifact: transform logic, fixtures, `workflow.yaml`, `codemod.yaml`, README, and package metadata must all describe the same codemod.
- Replace scaffold boilerplate before finishing. Do not leave generic README text, placeholder fixture intent, or mismatched usage descriptions in place.
- Use explicit workflow `base_path`, `include`, and `exclude` globs that match the actual target file types and keep `codemod.yaml` `targets.languages` aligned with that scope.
- Preserve the scaffold-selected package manager in `package.json` scripts and package-local README/development commands. Infer it from the scaffold choice, lockfile, or existing package metadata; do not rewrite `yarn`/`pnpm`/`bun` packages to `npx`/`npm` unless the user explicitly asked.
- Preserve repository-local package and lockfile conventions. In existing monorepos, do not introduce ad hoc dependency ranges or unrelated lockfile churn.
- Treat fixture quality as a release gate. Cover realistic positive cases, edge cases, preserve/no-op cases, and negative cases where similar code must stay unchanged.
- Do not stop while `validate_codemod_package` still reports starter scaffold markers, missing package surface updates, missing real test cases, or failing default tests.
- For reusable authored codemods, do not default registry access/visibility to private unless the user explicitly asked for a private package.
- Leave missing package author metadata to the CLI defaults/publish-time auth fallback unless the user supplied an explicit author.
- Do not create commits or push branches for codemod authoring/evaluation unless the user explicitly asked for git operations.

## Runtime flow

1. Discover candidates with `codemod search`.
2. Read the selected package's README/docs and perform any documented prerequisites or setup steps.
3. Run workflow-capable packages with `codemod run --dry-run` before apply.
4. Run `codemod <package-id>` and accept the install prompt when a package exposes installable skill behavior.
5. Enforce verification with tests and dry-run summaries before apply.

## Mandatory first action for migration/update/upgrade requests

- Run `codemod search` before proposing a manual migration plan.
- Do not jump straight to a handcrafted migration approach until registry discovery has been attempted and summarized.
- If a suitable existing codemod is found, prefer evaluating it with `--dry-run` before proposing bespoke manual or AI-only migration work.
- Only skip registry discovery when the user explicitly says not to use Codemod or not to search for existing codemods.

## First-turn behavior

- Before broad repo inspection or planning, derive a small set of high-signal search terms from the user request and run `codemod search`.
- For codemod authoring, inspect only a small representative slice of the repo after or alongside registry discovery, then scaffold and iterate.
- If the search returns a plausible existing package, the next step is to inspect that package's README/limits and run a dry-run, not to draft a manual migration plan.

## Anti-patterns to avoid

- Do not start by planning a manual migration when the request is an upgrade, update, or migration and the registry has not been searched yet.
- Do not create a new codemod package before checking whether an existing registry package already covers the migration.
- Do not keep reading broad guidance after a registry miss without scaffolding a package.
- Do not run a discovered package blindly without first reading its README/docs for prerequisites, config, and known limits.
- Do not introduce a shell step just to reach or mutate another related file path when JSSG can handle the hop.
- Do not use JSSG as a file-walking wrapper around parser-backed files while doing the actual transformation with `fs`, `root.source().replace(...)`, or full-source `.split(...)`/`.join(...)` rewrites.
- Do not choose Python, shell, or Node file I/O for JSON, YAML, XML, TOML, or source-code transforms when a matching `js-ast-grep` language exists.
- Do not stop codemod authoring only because Codemod MCP is unavailable; use `codemod ai` CLI equivalents.

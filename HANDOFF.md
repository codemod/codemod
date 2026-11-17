# Handoff: the missing JavaScript lockfile formats

Branch: `handoff/js-lockfiles`. Code head below this file: `8457034`, one commit on top of
`006544f` (#2494's code). No PR yet.

## This branch

Goal: before this, `dependencyGraph` read package-lock v2/v3, Yarn berry and pnpm v6 to v9. This
adds the formats still in wide use.

`8457034 feat(deps): read Yarn 1, package-lock v1, npm-shrinkwrap, pnpm v5 and bun.lock`:

- `yarn.lock` in both formats. A Yarn 1 lockfile lists no workspaces, so they come from the root
  `package.json`'s `workspaces` globs.
- `package-lock.json` v1 and `npm-shrinkwrap.json`, read like v2 and v3. When both sit in one
  directory, `package-lock.json` is skipped, as npm does.
- `pnpm-lock.yaml` lockfile 5 (`/name/1.2.3` keys with `_peer@1.0.0` suffixes).
- `bun.lock` (JSON with trailing commas), with workspaces and nested install paths resolved the
  way Node resolves them.
- The binary `bun.lockb` is not read; it is listed in `skipped` with the command that writes
  `bun.lock`, unless a `bun.lock` sits beside it.

Specs: `crates/deps/tests/specs/javascript.rs` (71 deps specs in total on this branch).

Review lessons checked: npm compare is total after #2494; npm names are exact; npm-family readers
keep a non-registry spec (`file:`, `git+https:`) as its literal text, for which `accepts()` already
returns `null`, so nothing else needed changing.

Known limits: `bun.lockb` (binary) is not read; no end-to-end workflow run on real Yarn 1 or bun
projects.

How to open this PR (after Dev's yes): push the code without the `HANDOFF.md` commit to a new branch, open the PR with base `claude/project-thread-x1q7lo` (#2494) once #2494 has been updated, and ask Mo to review.

```bash
git push origin handoff/js-lockfiles~1:refs/heads/deps/js-lockfiles
```

Next steps: Dev's yes to open it; after earlier language PRs merge, rebase onto #2494's branch and resolve the shared-line conflicts (keep both sides' entries).

## The whole stack at a glance

Goal: Mo's code-first workflow engine (`engine/code-first`, PR #2485) can read a repository's
lockfiles into a dependency graph (`dependencyGraph`), look up releases (`registry`) and compare
versions (`semver`). It started with npm (package-lock, Yarn berry, pnpm v6 to v9) and Cargo.
This work moves that API to its own module, `codemod/deps`, adds Python, then Go, Java/Kotlin,
PHP, Ruby and the missing JavaScript formats, and fixes what two reviews found.

Every branch below is code commits plus one top commit that only adds this `HANDOFF.md`.
The `HANDOFF.md` commit is for reading, not for merging: drop it before the code goes into a PR
(each branch's own section gives the exact push command).

Stack order (each builds on the one above):

| # | Handoff branch | Code head (below HANDOFF.md) | Maps to | Open PR branch today |
|---|---|---|---|---|
| 0 | (Mo's engine) | `engine/code-first` | #2485 | `engine/code-first` |
| 1 | `handoff/2492-review-fixes` | `634d187` | #2492 (root lockfiles by default) | `claude/project-thread-oqdqek` at `c783d72` |
| 2 | `handoff/2493-deps-split` | `41ee17a` | #2493 (move the API to `codemod/deps`) | `claude/project-thread-x1q7lo-deps` at `b1839b5` |
| 3 | `handoff/2494-python-lockfiles` | `006544f` | #2494 (Python) | `claude/project-thread-x1q7lo` at `1208b74` |
| 4a | `handoff/go-lockfiles` | `4b99e20` | new PR, base #2494 | none yet |
| 4b | `handoff/java-lockfiles` | `4b013cf` | new PR, base #2494 | none yet |
| 4c | `handoff/php-lockfiles` | `9d50d52` | new PR, base #2494 | none yet |
| 4d | `handoff/ruby-lockfiles` | `b0a3c76` | new PR, base #2494 | none yet |
| 4e | `handoff/js-lockfiles` | `8457034` | new PR, base #2494 | none yet |

The five language branches each sit directly on #2494's code, side by side, not on each other.
They touch the same shared lines (the `LOCKFILES` list, the `Ecosystem` enum, the arms in
`semver.rs` and `registry.rs`, `deps.d.ts`, the docs page), so whichever merges second needs a
small, mechanical rebase. Suggested merge order: Go, Java, PHP, Ruby, JS.

All commits are authored and committed as Dev Agrawal, with no other attribution, so the CLA
check passes.

### Where the code lives

- `crates/deps/` (crate `codemod-deps`): lockfile readers in `src/lockfiles/<format>.rs` and the
  dispatch in `src/lockfiles/mod.rs`; the graph model in `src/model.rs` (`Ecosystem`, `Node`,
  `Edge`); lookups in `src/walk.rs`; version rules per ecosystem in `src/semver.rs`; registry
  fetching and parsing in `src/registry.rs`. Specs: `tests/specs.rs` plus one file per language
  in `tests/specs/`.
- `crates/workflow/` (crate `codemod-workflow`): the engine. `src/js/deps.ts` and
  `src/js/dependencies.ts` are the JS side of `codemod/deps`; `src/deps_host.rs` and
  `src/tasks/dependency_graph.rs` bridge to Rust; `build.rs` checks imports.
- `crates/cli/npm/types/modules/deps.d.ts`: the public types for `codemod/deps`.
- `docs/community/workflows/reference.mdx`: the user-facing reference.

### How to run and verify (any branch)

```bash
git fetch origin <handoff-branch> && git checkout <handoff-branch>
export CARGO_INCREMENTAL=0
cargo fmt --check -p codemod-deps -p codemod-workflow
cargo clippy -p codemod-deps -p codemod-workflow --all-targets -- -D warnings
cargo test -p codemod-deps                      # all lockfile, version and registry specs
cargo test -p codemod-workflow --test tasks     # engine task specs (dependencyGraph etc.)
cargo test -p codemod-workflow --test engine    # build checks, imports, workflow types
(cd crates/cli/npm && pnpm install && pnpm run typecheck)
```

Gotcha if you keep several worktrees: they share one `target/` and cargo can link a stale
`codemod-deps` or `codemod-sandbox` built from another worktree (symptom: "no field named
`codemod_modules`" or a missing function). Either use a separate `CARGO_TARGET_DIR` per worktree
or `touch` the sources of `crates/deps`, `crates/workflow`, `crates/codemod-sandbox` and
`crates/llrt-capabilities` before building.

### Results on Nov 17, 2026 (local, all green)

| Branch | fmt | clippy -D warnings | codemod-deps specs | workflow tasks | workflow engine | npm typecheck |
|---|---|---|---|---|---|---|
| 2492 fixes | ok | ok | 26 | 42 | 43 | ok |
| 2493 split | ok | ok | 26 | 43 | 45 | ok |
| 2494 Python | ok | ok | 53 | 43 | 45 | ok |
| Go | ok | ok | 67 | 43 | 45 | ok |
| Java | ok | ok | 67 | 43 | 45 | ok |
| PHP | ok | ok | 67 | 43 | 45 | ok |
| Ruby | ok | ok | 65 | 43 | 45 | ok |
| JS | ok | ok | 71 | 43 | 45 | ok |

### Not tested anywhere in this stack

- GitHub CI has not run on any commit newer than the open PR heads (`c783d72`, `b1839b5`,
  `1208b74`); nothing here was pushed to a PR branch.
- The Node host test for `codemod/deps` (needs the `codemod-node` addon built) was not run.
- `cargo check -p codemod` (the full CLI) and repo-wide `pnpm lint` were not re-run after the
  review fixes; they passed earlier on the Python branch.
- No live registry calls for Go, Maven Central, Packagist or RubyGems: their release parsers are
  tested on fixture documents only. Live PyPI was fetched once, before the review fixes.
- An end-to-end workflow on real projects was run for Python only (uv, Poetry, Pipenv), and
  before the review fixes; not re-run since. No end-to-end runs for the five new languages.

### Decisions waiting on Dev (with my recommendation)

1. **Update the three open PRs from these branches.** #2492 takes one fast-forward commit;
   #2493 and #2494 need `--force-with-lease`, because their base moved when the fixes below them
   landed. Recommendation: yes; nobody else has pushed to those branches (checked Nov 17, 23:00 UTC).
2. **Open the five language PRs**, each based on #2494's branch, with Mo as reviewer.
   Recommendation: yes, all five at once, so Mo sees the shared pattern; merge in the order above.
3. **Go `isBreaking` is major-only** (`v0.29.0` to `v0.30.0` is not breaking). It falls out of the
   leading `v`, not a deliberate rule. Recommendation: strip the `v` and use the same caret rule
   as every other ecosystem, since Go promises nothing for v0 minor bumps.
4. **Clean break vs deprecated re-export** for the old `codemod/workflow` imports (asked of Mo in
   #2493). Recommendation: keep the clean break; old imports now fail with an error that names the
   new import, in both the workflow file and helper files.

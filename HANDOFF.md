# Handoff: #2494, Python lockfiles, PEP 440 and PyPI

Branch: `handoff/2494-python-lockfiles`. Code head below this file: `006544f`, five commits on top
of `41ee17a` (the `codemod/deps` split). The open PR #2494 (`claude/project-thread-x1q7lo`) is at
`1208b74`, which predates both review rounds and the rebase.

## This branch

Goal: Python projects get the same `codemod/deps` support as npm and Cargo: the graph from their
lockfile, version ranges compared by PEP 440, and releases from PyPI. It is also the pattern the
five other languages follow (one reader file, one `Ecosystem` arm, one spec file each).

Commits:

1. `47b424c feat(deps): read uv.lock, poetry.lock and Pipfile.lock` (`crates/deps/src/lockfiles/python.rs`).
   - uv.lock: every package and its edges; the project's own ranges come from each workspace
     member's `[package.metadata]`. Third-party packages' ranges are not in uv.lock, so their edges
     have no range.
   - poetry.lock: packages, edges and ranges; the project itself comes from `pyproject.toml`
     (PEP 621 `[project]`, PEP 735 `[dependency-groups]` and Poetry's tables; every group is `dev`).
   - Pipfile.lock: installed versions only, no edges between packages; the project's ranges come
     from `Pipfile`, so only packages the Pipfile names are reachable from the root.
   - Names are PEP 503-normalized (`Typing_Extensions` is `typing-extensions`).
2. `3813967 feat(deps): compare Python versions and ranges by PEP 440` (`pep440_rs` 0.7,
   Apache-2.0 OR BSD-2-Clause), plus Poetry's `^`, `~` and `||`.
3. `6dba668 feat(deps): PyPI releases for Python packages` (`registry.pypi`, PyPI JSON API). PyPI
   lists dependencies per file, not per release, so `releases({ accepting })` throws for Python.
4. `01d40a7 fix(deps): read Poetry prerelease bounds and marker splits, resolve by range` (first
   review round): `^1.2rc1` and epoch bounds, overflow at the largest number, a name locked at
   several versions resolves to the one its range allows, marker-split constraints keep every range,
   `isBreaking` takes the ecosystem (`0.1rc1` to `0.1` is not breaking), PyPI versions with no files
   are dropped.
5. `006544f fix(deps): total version order, PEP 503 lookups, no range for git and path sources`
   (second, independent review). Each has a spec in `crates/deps/tests/specs/python.rs`:
   - Sorting PyPI releases panicked on old non-PEP 440 versions ("user-provided comparison function
     does not correctly implement a total order"). `compare` is now total for every ecosystem:
     versions it cannot read sort before the ones it can, by text. The spec reproduces the panic on
     the old code.
   - `graph.node("PyYAML")`, `g.dependents("typing_extensions")` and other spellings PyPI accepts
     now find the normalized node (`Ecosystem::package_name`, used by `walk.rs`). npm and Cargo
     names stay exact.
   - Git, URL, path and editable dependencies have no range (`null`) instead of `*` in pyproject,
     Pipfile, uv metadata and PEP 508 `name @ url`, so `edge.accepts()` says "cannot tell" rather
     than "yes".
   - Poetry ranges written `>=1.2 <2.0` (space as AND) and `>=1.2 | <0.5` (one bar as OR) now read.
   - `isBreaking`: `0.0` and `0.0.0`, or `1.0` and `v1.0`, are the same release; an epoch change
     (`1.0` to `1!1.0`) is still breaking.
   - Pipfile.lock: the project installed as editable (`-e .`) is the root rather than a second
     workspace beside `<.>`; git sources take their `ref` as version; a wheel path is a package, not
     a workspace; custom package categories are read, as `dev`.
   - poetry.lock without a readable `pyproject.toml` is an error (lands in `skipped`) instead of a
     graph with no root.
   - PyPI release versions are normalized as lockfiles write them (`2.0.0.RC1` is `2.0.0rc1`), and
     PyPI releases carry no dependency groups instead of empty ones that read as "no dependencies"
     (typed as `ReleaseVersion`).

Visible behaviour change for reviewers: a uv workspace's edge to its own editable member now has
range `null` instead of `*`.

Tested on this branch: fmt, clippy, 53 deps specs, 43 task specs, 45 engine specs, npm typecheck.
Before the second review round also: real lockfiles produced by `uv lock`, `poetry lock` and
`pipenv lock` parse correctly; a live PyPI fetch; an end-to-end "outdated report" workflow on real
uv, Poetry and Pipenv projects through the CLI. Not tested: those three were not re-run after
`006544f`; no spec uses a real uv.lock with a forked (two-version) package.

How to update #2494 from this branch (after Dev's yes; force needed because the base moved):

```bash
git push --force-with-lease=claude/project-thread-x1q7lo:1208b74 \
  origin handoff/2494-python-lockfiles~1:claude/project-thread-x1q7lo
```

Next steps: Dev's yes; re-run the real-project demo once on the pushed code; then the five
language PRs, which are built on this branch's code.

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

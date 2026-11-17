# Handoff: cross-toolchain dependency graph prototypes

Branch `handoff/dep-graph-prototypes`, cut from `main` at c51c49a. The code is in
`prototypes/dep-graph/`. Nothing here is meant to merge into `main`. This branch is a record of the
prototype that came before `crates/deps`, plus what it still has that `crates/deps` doesn't.

## Goal

The idea is to make dependency analysis work like ast-grep does for code. ast-grep gives one query
shape over every language's syntax tree. This gives one graph shape over every toolchain's
lockfile, so the same questions work in any ecosystem:

- Who reaches package X, and through which chain?
- What refuses version V of X?
- What has no release that accepts V?
- What would bumping X to V take, and which files does it touch?

The motivating case was the Backstage React 18 → 19 case study. Its dependency numbers came from
throwaway Node scripts, and the enterprise dependency index couldn't answer it. That index keeps
resolved versions and one parent per package. It keeps no declared ranges, dependency kinds or peer
dependencies.

## Where things stand (Nov 17)

The prototype (`depq`, Nov 12) proved the model. That model then shipped in the workflow v2 engine:

| PR | State | What it is |
| --- | --- | --- |
| #2486 | merged into `engine/code-first` (Nov 17) | New `crates/deps` crate: Yarn Berry, pnpm, package-lock and Cargo parsers into one graph. Nodes are `name@version`, and every edge keeps its range and kind. Peer ranges are edges. It also adds a `dependencyGraph({ lockfiles }, callback)` step with a live graph handle, `registry.npm` / `registry.cargo`, and `semver`. Its Backstage demo reproduces depq's numbers. |
| #2492 | open, base `engine/code-first` | Reads only the target's root lockfiles by default; nested ones need a glob. package-lock resolves through every ancestor `node_modules`. A pnpm peer installed with a package becomes a peer edge, not a prod edge. |
| #2493 | open, stacked on #2492 | Moves `dependencyGraph`, `registry` and `semver` from `codemod/workflow` to a new `codemod/deps` module. Importing them from the old place is a build error that names the new one. Open question for Mo in the PR: keep a deprecated re-export or not. |

How each depq piece maps onto those PRs:

| depq | In #2486–#2493 |
| --- | --- |
| Shared model and parsers (`model.rs`, `parse.rs`) | `crates/deps` (`model.rs`, `lockfiles/*`), extended with package-lock and multiple lockfiles |
| `Registry` trait, curl + disk cache | `Services::package_registry`: one cached, deduplicated client per run, recorded by the step |
| Variant A: pattern strings (`ws:* ~> npm:x`) | TypeScript methods on the graph handle: `dependents`, `chains`, `workspacesReaching`, `releases({ accepting })`, `accepts(v)` |
| Variant B: SQL | Not carried over |
| Variant C: `impact` (blockers → fixes → files) | The Backstage demo workflow in #2486 (`upgradeBlockers`, about 150 lines of TS). It also checks that each fix brings in no new blocker, which depq doesn't do. |
| `Verdict::LocalOverride` (pnpm overrides) | **Not carried over.** Searching `crates/deps` on #2493's head for "override" finds nothing. |

## The three variants

All three share one Rust crate, `prototypes/dep-graph/depq` (1,372 lines plus 11 specs). The
release binary is 3.9 MB, mostly bundled SQLite.

### The shared model

- **Nodes:** ecosystem + `name@version`, either a workspace (with its directory) or third-party.
- **Edges:** resolved target (none for peers), declared range and kind
  (prod / dev / peer / optional / build).
- **Parsers:** Yarn Berry, pnpm v9 and Cargo.lock.
- **Workspace kinds:** Yarn's lockfile merges a workspace's dependencies and devDependencies, so
  depq reads `package.json` to tell them apart. Cargo kinds come from each member `Cargo.toml`,
  including `workspace = true`.
- **Registry:** behind a trait, so the host decides what the network is.
  - npm: the abbreviated packument.
  - crates.io: the sparse index. Cargo.lock keeps no ranges for third-party crates, so a
    `refuses` query fetches them only for the edges it looks at.

### A. Pattern CLI

`depq <repo> query '<pattern>'`, plus `stuck` to add registry verdicts.

- **Selectors:** `ws:` (the repo's own packages), `ext:` (third-party), `npm:` / `cargo:`, globs,
  and an optional `@range` on the resolved version.
- **Arrows:** `->` is one edge and `~>` is a path. Kinds go in brackets: `-[peer]->`.
- **Conditions:** `refuses V` and `accepts V`.

```
ws:* ~> npm:react-beautiful-dnd                 # who reaches it, shortest chain per workspace
ext:* -[peer]-> npm:react refuses 19.0.0         # third-party peers that refuse React 19
ws:* ~> cargo:bitflags@^1                        # who still pulls bitflags 1
```

### B. SQL

`depq <repo> sql <file.sql>`. The graph is loaded into in-memory SQLite as `nodes` and `edges`.
`satisfies(eco, req, version)` and `first_release_accepting(eco, pkg, current, dep, target)` are
host functions callable from SQL. The queries live in `prototypes/dep-graph/queries/`:

- `bump-plan.sql`: every React 19 blocker with its fix, and how many workspaces it reaches
  directly and transitively.
- `refuses.sql`
- `reach.sql`
- `cargo-dupes.sql`

SQL handles sets well and paths badly: `reach.sql` returns hop counts, not chains.

### C. Impact

`depq <repo> impact eco:name@V [--also a,b] [--ignore glob,...]` returns one JSON answer, shaped
like an MCP tool's:

- the repo's own manifests that refuse V;
- each third-party blocker with its verdict: fixable (and whether breaking), stuck, or local
  override;
- how many workspaces it reaches, directly and transitively, with an example chain;
- the files that import the package it arrives through, plus a Grep pattern for them.

## Numbers

Backstage at 39f85f35, compared with the Nov 10 case study:

| | Case study | depq (A, B and C agree) |
| --- | --- | --- |
| Workspaces with `@material-ui/core` direct | 37 | 37 |
| Chain to react-beautiful-dnd | core-components → @material-table/core | same |
| Lockfile packages whose react / react-dom peer excludes 19 | 33 | same 33 (diffed by name) |
| Third-party blockers (minus React itself and Yarn plugins) | 26 | 26 |
| Fixed by a bump / no React 19 release | 11 / 15 | 11 / 15 |
| Breaking fixes | 6 majors | 7 (`@graphiql/react` 0.29 → 0.30 counts, since a 0.x minor is breaking) |
| Published packages peering on React below 19 | 55 | 55 (57 counting private) |
| Files importing a blocker's entry package | ~545 `@material-ui/core` mentions (plain grep) | 528 for MUI core; 595 across all blockers (#2486's step gets 594) |

On codemod at 868127b (pnpm and Cargo in one graph):

- 1,627 nodes.
- 107 crates are locked in more than one version.
- Bumping `tree-sitter` to 0.27.2: two of codemod's own crates pin `0.26` (codemod-ai,
  testing-utils). Three ast-grep crates refuse it, and ast-grep 0.45.3 fixes all three. That
  answer came from live crates.io lookups, which #2486's test plan lists as not run.

Speed on Backstage:

| Step | Time |
| --- | --- |
| Parse (4,053 nodes, 13,564 edges) | ~90 ms |
| Path query | ~5 ms |
| SQL script, including load | 27–113 ms |
| `impact`, warm registry cache | 0.4–0.5 s |
| `impact`, cold registry cache (Nov 12) | 2.8 s |

All outputs are in `prototypes/dep-graph/results/`. Today's rerun used registry answers cached on
Nov 12.

## Tested

- 11 specs in `depq/tests/specs.rs`, one rule each (output in `results/spec-output.txt`):
  - Yarn kinds from package.json
  - shortest chains
  - `refuses` / `accepts`
  - pnpm peer-suffix collapse
  - Cargo versioned dependencies
  - registry verdicts: fixable, stuck, 0.x breaking, local override
  - pattern errors
- On Nov 17 the specs pass on this branch (`cargo test --release` inside
  `prototypes/dep-graph/depq`), and `demo.sh` reproduces the Backstage and codemod results above.
- A private pnpm + Cargo repo was also run. There, most apparent blockers of a Vite major came from
  the repo's own pnpm `overrides`, which is what motivated `local_override`. That output isn't on
  this public branch.

## Not tested

- **Lockfile formats:** npm package-lock, Yarn v1, bun, Go, Python and Gradle.
- **Yarn and pnpm details:** Yarn virtual packages, and pnpm catalogs beyond what the lockfile
  resolves.
- **Cargo details:** features, and renamed dependencies beyond the basic case.
- **Registry verdicts:** checked only against the Backstage numbers and two spot checks (vitest
  4.1.12, ast-grep-core 0.45.3).
- **File matching:** line-based import matching, not ast-grep.
- **Integration:** none with the enterprise index or the MCP.

## How to run

You need cargo (the repo's `rust-toolchain.toml` applies), git, curl, and network access to
github.com, registry.npmjs.org and index.crates.io. `depq` has its own `[workspace]`, so it builds
without joining the root workspace.

```bash
cd prototypes/dep-graph/depq
cargo test --release                       # the 11 specs
cargo build --release
D=target/release/depq

$D <repo> query 'ws:* ~> npm:react-beautiful-dnd'                 # A
$D <repo> stuck 'ext:* -[peer]-> npm:react refuses 19.0.0'        # A + registry
$D <repo> dupes cargo                                             # duplicates
$D <repo> sql ../queries/bump-plan.sql                            # B
$D <repo> impact npm:react@19.0.0 --also react-dom \
   --ignore 'react,react-dom,react-test-renderer,react-reconciler,@yarnpkg/*,ink-*'   # C
$D <repo> export > graph.json                                     # the raw graph

# Everything in results/, including a Backstage clone at the case-study commit:
bash ../demo.sh <workdir> <codemod checkout> [<other repo> <eco:name@version>]
```

- Add `--offline` to skip the registry. Registry answers are cached in `~/.cache/depq`; use
  `--cache DIR` to put them elsewhere.
- Timings go to stderr and results to stdout.

## Decisions waiting on Dev

1. **Retire depq.** `crates/deps` has the same model and is in the product. Leave this branch as
   the record, don't merge it, and don't keep the two in step. *Recommended.*
2. **Port the pnpm override check into `crates/deps`.** When a pnpm peer range refuses V but the
   installed release's published range accepts it, report it as the repo's own override. Without
   this, a pnpm repo with `overrides` shows false blockers and wrong "fix" versions. It should be a
   small change on top of `releases()`, plus one spec. *Recommended next.*
3. **Leave the pattern strings (A) and SQL (B) unported.** The TypeScript graph handle covers
   workflows. Revisit the pattern syntax only if Grep or the MCP needs a one-line text query form
   for dependencies. *Recommended.*
4. **Never answered (Nov 12):** "does the code join fit what you want to show Mo?" #2486's
   Backstage demo has since done that job, so this can be dropped.

## Questions for Mo

- **#2493:** keep a deprecated re-export from `codemod/workflow`, or make a clean break? The
  question is asked in the PR.
- **Enterprise index:** should it keep every edge with range, kind and peers, as `crates/deps`
  does, so Grep, Insights and the MCP can answer "refuses V"? I don't know whether this has been
  asked yet.

## Next steps

1. Land #2492, then #2493.
2. Port the override check (decision 2) into `crates/deps`, after #2493.
3. Gaps #2486 lists:
   - a run-wide registry rate limit;
   - `registry` inside jssg transforms;
   - the Node.js host;
   - Yarn v1, bun, Go and Python lockfiles.
4. Run `dependencyGraph` against a real Cargo repo with live crates.io. depq's tree-sitter result
   above can serve as the expected answer.
5. Decide the enterprise-index question with Mo.

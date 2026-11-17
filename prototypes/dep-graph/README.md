# Cross-toolchain dependency analysis: prototypes

Superseded by `crates/deps` (#2486). See `HANDOFF.md` at the repo root for how they relate and what's still open.

The idea: what ast-grep is for syntax trees, this is for dependency graphs. Every toolchain's lockfile
maps into one graph shape. The same few questions then work in any ecosystem, and the answers join to
code.

All three variants share one Rust crate, `depq/`. The parsers are the "grammar" layer and shouldn't
differ between variants. What differs is how you ask the question and what comes back.

```bash
bash demo.sh <workdir> <codemod checkout> [<other repo> <eco:name@version>]   # builds, runs the specs, reruns results/
```

## The shared model (`depq/src/model.rs`, `parse.rs`)

- **Node:** ecosystem + `name@version`. A node is either a workspace (the repo's own package, with
  its directory) or third-party.
- **Edge:** from → resolved target. Each edge keeps the declared range, the kind
  (prod / dev / peer / optional / build) and its source workspace.
- **Parsers:** Yarn Berry, pnpm v9 (multi-document lockfiles, peer-suffixed snapshot keys, `link:`
  workspaces, aliases) and Cargo.lock. Yarn takes workspace dependency kinds from each
  `package.json`. Cargo takes them from each member `Cargo.toml`, including `workspace = true`.
- **Registry gap:** Cargo.lock keeps no requirements for third-party crates. A `refuses` query
  fills just the edges it needs from the crates.io sparse index.
- **Network:** the registry sits behind a `Registry` trait, so the host decides what the network is.
  The CLI uses curl with a disk cache. Tests use a fake.

## The variants

| | What it is | Answers | Size / speed |
| --- | --- | --- | --- |
| **A. Pattern CLI** | `depq <repo> query 'ws:* ~> npm:react-beautiful-dnd'`. Selectors (`ws:`, `ext:`, `npm:`, `cargo:`, globs, `@range`), `->` for one edge, `~>` for a path, `-[peer]->` for kinds, `refuses V` / `accepts V`. `stuck` adds registry verdicts. | Who reaches X and through which chain. What refuses V. Duplicates. | Backstage yarn.lock parses in ~90 ms (4,053 nodes, 13,564 edges). A path query takes ~5 ms. |
| **B. SQL** | The same graph as `nodes` / `edges` in in-memory SQLite. `satisfies()` and `first_release_accepting()` are host functions you can call from SQL. Queries are in `queries/*.sql`. | Refusers and the whole bump plan in one query (`bump-plan.sql`). Reach as a recursive CTE. Cargo duplicates with who pulls each version. | 27–116 ms per script, including load. |
| **C. Impact (the MCP tool shape)** | `depq <repo> impact npm:react@19.0.0`. It joins refusers, registry fixes and reach with the code: the files importing the package through which each blocker arrives, plus a Grep pattern for them. It also lists the repo's own manifests that refuse V. | "MUI v4 blocks React 19, it's in 37 workspaces, 528 files import it, and no release fixes it." | 0.37 s warm, 2.8 s with a cold registry cache. |

Size: 1,372 lines of Rust plus 4 SQL files and 11 specs. The release binary is 3.9 MB, mostly
bundled SQLite.

## Checked against the Backstage case study (yarn, commit 39f85f35)

| Case study | depq | |
| --- | --- | --- |
| `@material-ui/core` direct in 37 workspaces | 37 (A, B, C) | match |
| react-beautiful-dnd via core-components → @material-table/core | same chain | match |
| react-side-effect reaches 3 workspaces | 3 | match |
| 33 lockfile packages whose react / react-dom peer excludes 19 | the same 33 (diffed by name) | match |
| 26 after dropping React itself and Yarn's plugins | 26 | match |
| 11 fixable by a bump, 15 with no React 19 release | 11 / 15 (A, B, C) | match |
| 6 of the fixes are majors | 7 breaking | `@graphiql/react` 0.29 → 0.30 also counts, because a 0.x minor bump is breaking under caret rules |
| 55 published Backstage packages peer on `^17 \|\| ^18` | 55 published (57 counting private) | match |
| ~545 files import `@material-ui/core` (a plain grep) | 528 | depq counts import lines inside the 37 workspaces that declare it, not every mention |

## Beyond yarn (live registry, Nov 12)

- **codemod (pnpm + Cargo, one graph):** 1,627 nodes. 107 crates are locked in more than one
  version. `bitflags` 1 comes in through `portable-pty` and through `ty_project → notify → kqueue`.
  - **Bumping `tree-sitter` to 0.27.2:** two of codemod's own crates pin `0.26` (codemod-ai,
    testing-utils). Three ast-grep crates refuse it, and ast-grep 0.45.3 fixes all three. I
    checked that one against the crates.io index.
- **A private pnpm + Cargo repo:** about 4,500 nodes. Most packages that seemed to refuse a Vite
  major were refused only because of the repo's own pnpm `overrides`. pnpm writes overrides into
  the peer ranges it records. C reports these as `local_override`. That run's output isn't on this
  public branch.

## What I'd pick

**C on top of A's engine.** The join is the product: graph answer → files → codemod. A's pattern
syntax is the right surface for a person or an agent typing a question, and C is what an MCP
`dependency_query` returns.

**B** is worth it only if Insights wants ad-hoc aggregates over snapshots. SQL handles sets well
but paths badly: `reach.sql` gives hop counts, not chains.

I didn't build a workflow v2 variant. Without a recorded `task()` step it would just be a shell step
calling this binary. With `task()`, the host hands the engine `read` and `fetch`, the same seam the
`Registry` trait already models.

## The question for the enterprise index

The enterprise dependency index already parses many lockfile formats, but keeps one parent per
package, resolved versions only, and no declared ranges, dependency kinds or peer dependencies.
Two things would be needed to answer "refuses V" there:

1. Every edge, with its range, kind and peers. That is the shape depq (and now `crates/deps`) keeps.
2. Cached registry access (npm, crates.io) for the "no release accepts V" answer.

## Tested

- 11 specs (`results/spec-output.txt`), covering:
  - Yarn kinds from package.json
  - shortest chains
  - `refuses` / `accepts`
  - pnpm peer-suffix collapse
  - Cargo versioned dependencies
  - registry verdicts: fixable, stuck, 0.x breaking, local override
  - pattern errors
- Full runs on Backstage and codemod (868127b). Outputs are in `results/`. A private repo was also run;
  its output is not on this branch.

## Not tested

- go.mod/go.sum, Python (uv, poetry), Gradle, npm package-lock, Yarn v1 and bun.
- Yarn virtual packages.
- pnpm catalogs, beyond what the lockfile already resolves.
- Cargo features, and renamed dependencies beyond the basic case.
- Registry verdicts beyond the Backstage comparison and two spot checks (vitest 4.1.12,
  ast-grep-core 0.45.3).
- File matching is line-based import matching, not ast-grep.
- Nothing here is wired into the enterprise index or the MCP. The workflow v2 version is
  `crates/deps` in #2486; see `HANDOFF.md` at the repo root.

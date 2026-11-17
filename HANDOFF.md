# Handoff: workflow v2 getting-started tutorial

Branch: `handoff/workflow-v2-tutorial` in `codemod/codemod`.
Last updated: 2026-11-17.

## Goal

Ship user-facing docs for the code-first workflow engine (workflow v2, `workflow.ts`) next to the
workflow v2 blog post. The missing piece was a tutorial: the reference, YAML upgrade, testing and
Node host pages already exist on `engine/code-first`.

The tutorial is one new page, `docs/community/workflows/getting-started.mdx` ("Build your first
workflow"). It grows one workflow that replaces `util.isArray` / `util.isBuffer`, in five sections:

1. Run a jssg transform over every TypeScript file with `forEachFile`
2. Return `{ edit, data }` from the transform and read `result.data` in the workflow
3. Run shell commands: list changed files from `git diff` (`result.stdout`), format them with Prettier
4. Decide what runs next: stop when nothing was replaced, ask for an `approval` before formatting
5. Edit files with an inline `forEachFile` callback (remove the unused `node:util` import)

## What is on this branch

The branch is `#2491`'s head (`b1feab9`) plus three commits, all authored as Dev Agrawal:

| Commit | What |
| --- | --- |
| `1654ae0` | The tutorial as written for #2489's API (same content as PR #2490), plus: nav entry in `docs/docs.json`, a card on `docs/community/workflows.mdx`, and the real `// Generated from workflow.yaml ...` comment line in `migrate.mdx` |
| `8aa876a` | Tutorial updated for #2491's API (sections 2 to 5 rewritten, real outputs) |
| this file | `HANDOFF.md` (delete before merging anything) |

The two docs commits also apply cleanly on `engine/code-first` alone (checked with a cherry-pick).

## How it relates to #2489, #2490 and #2491

All three are against `engine/code-first` (Mo's workflow v2 branch, PR #2485), not `main`.

- **#2489** (merged into `engine/code-first` 2026-11-17, squash): removed workflow state. Its API had
  shell steps write `key=value` lines to `$CODEMOD_OUTPUT` (read back as `result.values`) and jssg
  `data` as counted label rows (`{ cardinality, count }`).
- **#2490** (draft, `claude/project-thread-1rougm`, head `a409b63`): the tutorial written for #2489's
  API. **It is stale.** Sections 2 to 5 don't run on #2491. Since #2489 was squash-merged, #2490 now
  also shows #2489's five original commits; it needs rebasing to carry only the docs commit.
- **#2491** (open, `claude/project-thread-qkxezu`, head `b1feab9`, conflicts with `engine/code-first`):
  shell steps return `{ stdout, stderr, exitCode }`; jssg `data` is any JSON, returned as
  `[{ file, data }]` sorted by path (or folded with `jssg(path, { reduce, initial })`); metrics and
  `$CODEMOD_OUTPUT` are removed. **This branch's tutorial is written for #2491.**

## What changed for #2491 (commit `8aa876a`)

Driven by an independent review (`/mnt/project-files/review-2026-11-17/docs-getting-started.md` in
the project files), checked again here:

- Section 2: the transform returns `data: edits.length` (calls replaced in that file). The workflow
  prints `${file}: ${data} replaced` from `migrated.data`. The `Metrics:` output block is gone
  (the CLI no longer prints it). A pointer to reducers (`/community/jssg/metrics#folding-data-as-files-finish`).
- Section 3: `shell("git diff --name-only")`, then `(changed.stdout ?? "").split("\n").filter(Boolean)`.
  `?? ""` is needed: `ShellResult` is `Skippable`, so `stdout` is `undefined` in a dry run and strict
  TypeScript rejects `changed.stdout.split`. The page says why. Section 3 runs with `--approve` and
  says a terminal run asks `Run shell command: ...? (y/N)` before each command.
- Section 4: stop when `migrated.data.length === 0` (a file with no replacements returns `null`, so it
  is left out of `data`).
- Section 5: starts with `git reset --hard HEAD~1`. Section 4 ends by committing the migrated code, so
  the old `git checkout .` left nothing to migrate.
- `cwd` is described as a `shell` option, not a `step` option.
- The callback sentence now says it can read the workflow's variables (copied in when the step
  starts) but can't assign to them. The old text claimed it read `SOURCES` "as frozen copies".
- Section 1 notes that a terminal run ends with `View results in the browser? (Y/n)`, which the
  page's outputs leave out.

## What was tested, and what wasn't

Tested on a debug build of #2491 (`b1feab9`, `cargo build -p codemod`, about 10 minutes from a warm
target dir):

- **Every section, exactly as the page says**, from a fresh `my-app` git repo: section 1, section 2,
  section 3 (`--approve`), section 4 (`--no-interactive` pause, `workflow resume --id ... --approve`,
  commit, rerun stops early), and section 5 (`git reset --hard HEAD~1`, run with `--approve`). Every
  output block on the page is pasted from these runs. The final `git diff` matches the page.
- The code was extracted from the page itself (not retyped), and the section 5 `workflow.ts` that ran is
  byte-identical to the page's last code block.
- `tsc --strict` on every section's `workflow.ts` and the transform, against #2491's
  `crates/cli/npm/types` (`codemod/workflow`, `codemod/ast-grep`, `langs/typescript`): passes.
- `codemod workflow validate` on the final package: passes.
- Section 3 in a real pseudo-terminal: the per-command `(y/N)` prompts and the closing browser prompt
  appear as described.
- `codemod workflow upgrade --dry-run` still writes the `// Generated from workflow.yaml by ...` line
  that `migrate.mdx` now shows.
- `mint broken-links` in `docs/`: no broken links.

Not tested:

- `mint dev` rendering (layout, `<Steps>`, line highlights such as `{15-18}`). `mint validate` needs
  network access and wasn't run.
- The published `npx codemod` package. The page says `npx codemod`, but no npm release contains
  workflow v2 plus #2491 (`codemod@2.0.0-alpha.3` is the #2485 engine, before #2489). Until a release
  ships, readers who copy the commands get an engine the page doesn't describe.
- The Node.js host (`codemod/testing`, `host: "node"`): the tutorial only uses the CLI.
- Windows. #2491's own review notes translated YAML state is `sh`-only; the tutorial uses `git`,
  `npx prettier` and a POSIX path style but no YAML.
- Other v2 pages were not re-run on #2491, except as noted below.

## Found along the way (not fixed here)

- `docs/community/workflows/reference.mdx` on #2491: the shell examples
  `JSON.parse(stdout.trim()...)` and `JSON.parse(detect.stdout)` fail `tsc --strict`
  (`'stdout' is possibly 'undefined'`), for the same `Skippable` reason as above. Either the
  examples add `?? ""`, or `ShellResult` stops being `Skippable` outside dry runs.
- Shell commands run in the directory the CLI starts in, not `--target` (same as v1). `cwd` is relative
  to the target. The tutorial works around this by running from inside `my-app`.
- PR #2491's description ends with a "Generated by Claude Code" footer that the PR tool added. Dev
  asked for no Claude attribution in PR text. (#2490's footer was removed on 2026-11-17.)

## Decisions waiting on Dev

1. **How to update #2490.** Options:
   - Rebase #2490 onto #2491's branch (`claude/project-thread-qkxezu`) with these two docs commits,
     force-push, and change its base to that branch, so it merges after #2491. **Recommended**: the
     diff stays docs-only and the page describes the API it ships with.
   - Fold the two docs commits into #2491 and close #2490.
   - Rebase #2490 onto `engine/code-first` with only the #2489-era commit (`1654ae0`), to merge now.
     Not recommended: sections 2 to 5 describe an API that #2491 removes.
2. **When the page goes live.** It should merge with, or after, the release that contains workflow v2
   plus #2491, or carry a version note in the PR description (not in the page).
3. **The reference page's shell examples**: fix the examples, or change the `Skippable` type.

## Next steps

1. Get Dev's pick on #2490 (decision 1). For the recommended option:
   ```bash
   git fetch origin claude/project-thread-qkxezu handoff/workflow-v2-tutorial
   git checkout -B claude/project-thread-1rougm origin/claude/project-thread-qkxezu
   git cherry-pick 1654ae0 8aa876a            # not the HANDOFF.md commit
   git push --force-with-lease origin claude/project-thread-1rougm
   ```
   Then set #2490's base to `claude/project-thread-qkxezu` and rewrite its description for #2491's API
   (steps 2 and 3 currently say `$CODEMOD_OUTPUT` and "counts").
2. Keep commits and PR text authored as Dev with no Claude attribution: check the committer too
   (`git log --format='%an | %cn'`), since a cherry-pick or rebase sets it from your git config.
3. When #2491 changes again, rerun the five sections (below) and update outputs.
4. Before merge: `mint dev` and look at the page.

## How to build and run things

Build the CLI from this branch (Rust; needs the toolchain from `rust-toolchain.toml`):

```bash
cargo build -p codemod        # binary: target/debug/codemod
```

Re-run the tutorial: create `my-app` and `util-is-to-native` side by side as the page's "Set up"
section says, copy each section's files from the page, and run the commands from inside `my-app`,
replacing `npx codemod` with the path to `target/debug/codemod`. Run ids in the outputs change per run;
everything else should match.

Type-check the samples against this branch's types:

```bash
mkdir -p tc/node_modules/codemod && cd tc
ln -s <repo>/crates/cli/npm/package.json node_modules/codemod/package.json
ln -s <repo>/crates/cli/npm/types node_modules/codemod/types
npm i typescript
# tsconfig: module/moduleResolution NodeNext, strict, noEmit, skipLibCheck, types: ["codemod"]
npx tsc -p .
```

Preview the docs (Mintlify; config is `docs/docs.json`):

```bash
cd docs
npx mint dev            # local preview
npx mint broken-links   # link check (works offline)
```

## Related material (project files, not in the repo)

- `/mnt/project-files/workflow-v2-docs/for-2491/`: the page and both docs commits as patches.
- `/mnt/project-files/review-2026-11-17/docs-getting-started.md`: the independent review this update
  follows, with code references into #2491.
- `/mnt/project-files/review-2026-11-17/pr-2491.md`: review of #2491 itself.
- The workflow v2 blog post is a separate task; keep its terms in line with this page
  (`step`, `forEachFile`, `jssg`, `shell`, `{ edit, data }`, `result.data`, `result.stdout`).

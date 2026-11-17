# Handoff: `codemod ai doctor` / `codemod ai uninstall` review fixes

Last updated 2026-11-17. This branch is a side branch so that nothing is local-only.
It is draft PR #2481 (branch `claude/project-thread-0n54sh`, head `269d1b3`) plus one more
commit with fixes from an independent review. **Don't merge this branch.** Bring its last commit
onto the PR branch instead (see "How this relates to #2481").

## Goal

Ship two new CLI commands for the Codemod AI setup that `npx codemod ai` installs into coding
agents (Claude Code, Cursor, Codex, OpenCode, Goose, Antigravity):

- `npx codemod ai doctor`: checks each agent Codemod is set up for and prints pass, warn or fail per
  check, with a "Fix:" line. It checks the skill, the commands, the MCP entry, the launcher (npx),
  that the MCP server starts and answers, and the agent's trust/approval. It exits 1 on a failure.
- `npx codemod ai uninstall`: removes what setup added for one agent and one scope (`--project`,
  the default, or `--user`). It lists the changes first, then asks for confirmation (or takes
  `--yes`). It also supports `--dry-run` and `--format json|yaml`. Anything it isn't sure is
  Codemod's goes under "Kept", with a reason.

Before #2481, both commands were unknown subcommands. A before/after recording is in the PR
description.

## Branch contents

| Commit    | Author | What |
| --------- | ------ | ---- |
| `86689e4` | Dev    | The feature: `doctor.rs`, `uninstall.rs`, `ai/setup_health.rs`, CLI wiring, docs in `docs/community/cli.mdx` |
| `299bc90` | Claude | Merge of `origin/main` at `c51c49a` |
| `269d1b3` | Claude | CI fix after main added the `sdk-migration` skill: doctor and uninstall handle both commands and both skills |
| `8c945ab` | Claude | **Review fixes (this handoff).** Not on #2481 yet |
| this doc  | Claude | `HANDOFF.md` only |

The commits up to `269d1b3` are exactly what is on #2481, and CI was green there (Rust test, lint,
format, Windows JSSG).

## What the review found and what `8c945ab` does

The review is in the project files at `review-2026-11-17/pr-2481.md`. Paths below are under
`crates/cli/src/commands/`.

| Finding | Problem | Fix in `8c945ab` |
| ------- | ------- | ---------------- |
| H1 | If `.claude/skills` is a symlink to `.cursor/skills`, uninstalling Claude deleted Cursor's skill, because the "shared folder" check compared path text | `plan_skill` and the discovery-guide check now use `same_existing_path`, which resolves symlinks |
| H2 | On Windows, `find_program` found the extensionless `npx` shell script before `npx.cmd`, so doctor said the MCP server couldn't start | Windows tries `.exe`, `.cmd`, `.bat`, then the bare name (`WINDOWS_PROGRAM_EXTENSIONS`, `find_program_in`) |
| M1 | Uninstall deleted the whole skill folder and the whole `<agent>/codemod` folder, and a command file the user had added notes to | It deletes only the files Codemod wrote and keeps the folder (listed under Kept) when other files remain. A command is removed only if it matches the installed text or the fingerprint in the install record. Otherwise it is kept as "it was edited" |
| M2 | Any MCP server named `codemod` was removed, even one that runs something else | Every config goes through `read_mcp_launch` + `launches_codemod`. A server that launches something else is kept, and so is a config it can't read |
| M3 | The plan was built before the "Remove these?" prompt and written after it, so it could overwrite newer edits (Claude Code rewrites `~/.claude.json` constantly). Writes weren't atomic | `handle_uninstall` re-plans right after confirmation. Writes go to a temp file, then rename |
| M4 | Run from the home folder, project and user scope are the same files, so `--user` found nothing | `installed_setups(paths, only: Option<InstallScope>)` lists only the asked-for scope when given |
| M5 | A Codemod section in `CLAUDE.md`/`AGENTS.md` whose end marker was deleted was skipped silently | It is listed under Kept: "its end marker is missing, so remove the Codemod section by hand" |
| Low | A dangling symlink at a path to delete was left behind | Deletes check `symlink_metadata`, so dangling links go too |

## Exactly what was tested

Run on Linux (this container). Nothing was run on Windows or macOS.

- `cargo test -p codemod --bin codemod`: **560 passed**, 0 failed (553 before `8c945ab`).
- `cargo clippy -p codemod --tests --no-deps -- -D warnings`: clean. This is the command CI runs.
  (`--all-targets` flags an unused `SDK_MIGRATION_VERSION_MARKER`, which is main's code, not this PR's.)
- `cargo fmt -p codemod -- --check`: clean.
- New one-rule specs. Each of the five uninstall specs was also run against `269d1b3`: all five
  **fail there and pass now**, so they test the fixes.
  - `uninstall_keeps_a_skill_another_agent_reaches_through_a_symlinked_skills_folder` (H1, unix only)
  - `uninstall_keeps_a_codemod_command_the_user_added_notes_to` (M1)
  - `uninstall_keeps_files_the_user_added_to_codemods_folders` (M1)
  - `uninstall_keeps_a_codemod_mcp_server_that_runs_something_else` (M2)
  - `uninstall_reports_a_codemod_section_whose_end_marker_was_deleted` (M5)
  - `uninstall_for_all_projects_finds_the_setup_when_run_from_the_home_folder` (M4)
  - `windows_lookup_picks_npx_cmd_over_the_shell_script_next_to_it` (H2, runs on every OS against
    the Windows extension order)
- The existing round-trip spec (`uninstall_leaves_the_project_and_home_folder_as_they_were_before_setup`,
  every agent and both scopes) still passes, so a normal install and uninstall still leaves no files behind.

## What was not tested

- **Windows itself.** H2 is tested only through the lookup order. Nobody has run `doctor` on a real
  Windows machine with Node installed.
- **The re-plan after the prompt (M3).** `setup_health.rs` has no tests, and the change is three lines.
  It was checked by reading only.
- **The temp-file-then-rename write** has no crash test.
- The review's suggested tests 8 (user-scope Claude uninstall keeps the other keys of `~/.claude.json`
  when an MCP entry was present) and 9 (`uninstall` with no `--yes` and no terminal removes nothing
  and exits 1) were not written.
- No manual run of the built binary after `8c945ab`. The before/after recording in the PR description
  is from the original feature commit.

## How to run and verify

```bash
cargo test -p codemod --bin codemod harness_adapter        # doctor + uninstall specs, ~1s once built
cargo test -p codemod --bin codemod                        # whole CLI bin
cargo clippy -p codemod --tests --no-deps -- -D warnings
cargo fmt -p codemod -- --check
```

To try it by hand in a throwaway folder:

```bash
cargo build -p codemod
mkdir /tmp/try && cd /tmp/try && git init -q
<repo>/target/debug/codemod ai --harness claude --project --no-interactive   # set up
<repo>/target/debug/codemod ai doctor
<repo>/target/debug/codemod ai uninstall --dry-run
<repo>/target/debug/codemod ai uninstall --yes
```

## Bugs still open

- **M6 (unverified):** doctor looks up `projects[<folder>]` in `~/.claude.json` using the native path.
  On Windows that has backslashes. If Claude Code writes forward slashes there, doctor always warns
  "hasn't approved". It's only a warning. It needs one check on Windows. If confirmed, also try the
  forward-slash form.
- **Low, not fixed:**
  - Doctor's MCP check kills `npx` but not the server it started, so a server that never answers can
    be left running.
  - When everything ends up under Kept, the heading still says "This removes Codemod from …" and
    the exit code is 0.
  - Empty parent folders are removed up to the project/home root, including empty ones the user had
    before setup (for example an empty `.cursor/commands`).
  - Removing the install record also drops tracking of skills installed with `install-skill`. The
    skills stay on disk untracked, and the output doesn't say so.

## Decisions waiting on Dev

1. **Bring `8c945ab` onto #2481?** Recommendation: yes. It fixes both high findings and five medium
   ones, and every new spec fails without it. The quickest way is to cherry-pick it onto
   `claude/project-thread-0n54sh` (it applies cleanly on `269d1b3`).
2. **Authorship / CLA.** `299bc90`, `269d1b3` and `8c945ab` are authored by Claude, so the CLA
   check flags them. Dev prefers commits authored as Dev. The session couldn't author as Dev.
   Recommendation: Dev re-authors with
   `git rebase --exec 'git commit --amend --no-edit --reset-author' 86689e4` on the PR branch, then
   force-pushes.
3. **Mark #2481 ready for Mo?** Recommendation: after 1 and 2, and after one manual run of
   `doctor` + `uninstall --dry-run` in a real project. M6 and the Lows can be follow-ups.
4. **Linear.** No Linear issue exists for doctor/uninstall (searched). Dev asks for all work to be
   tracked there. Recommendation: one CDMD issue linking #2481, with M6 and the Lows as its open items.
5. **Old branch `claude/project-thread-8vg0rd`.** It holds the original feature commit and is no
   longer used. Recommendation: delete it once #2481 merges.

## How this relates to #2481

- #2481 is a draft, head `269d1b3`, CI green, not yet reviewed by Mo.
- This branch = #2481 + `8c945ab` (+ this file). Don't open a PR from this branch. Move `8c945ab`
  onto the PR branch (decision 1).
- Leave `HANDOFF.md` out of #2481. It's for whoever picks this up, not for the repo.

## Next steps

1. Dev decides 1 and 2. Then cherry-pick `8c945ab` onto the PR branch, re-author, push, and check CI.
2. Run doctor/uninstall by hand once in a real project (Claude Code and Cursor). If possible, do one
   run on Windows for H2 and M6.
3. Optionally add the review's tests 8 and 9.
4. Mark #2481 ready and ask Mo for review. Track it in Linear.
5. Follow-ups: M6, the four Lows.

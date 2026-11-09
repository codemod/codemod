# `npx codemod ai` first-run UX plan

Dogfood notes from running the published CLI (`codemod@latest`) as a first-time user: help text, non-interactive install, auto harness detection, Cursor/Claude artifacts, list/docs/tools/call/feedback, and reinstall. Goal: an exceptional first session with no bugs or avoidable friction.

## Verdict

Install mostly works, but the first run still feels like an internals dump more than an onboarding. The biggest gaps are: fragile MCP wiring, silent harness/scope defaults, suppressed warnings, weak post-install guidance, and a command surface that mixes “set me up” with “agent toolchain” without a clear human path.

## What works today

- One command path is discoverable from docs and the bare-CLI next-steps list.
- Non-interactive install for Cursor/Claude finishes quickly and prints a readable success block.
- Skill + MCP config + `/codemod` command + discovery-guide upserts land together.
- CLI fallbacks (`docs`, `dump-ast`, `node-types`, `tools`, `call`) work without MCP.
- `ai list` correctly reports installed skills once the right harness is selected.

## Gaps found while dogfooding

### P0 — breaks trust or first success

1. **MCP config pins a disposable binary path**
   - Observed: `.cursor/mcp.json` / `.mcp.json` used an absolute path under `~/.npm/_npx/.../codemod`, not the documented `npx codemod@latest mcp`.
   - Why it hurts: cache cleanup, OS upgrades, or a new machine break MCP with no obvious recovery. Docs and install diverge.
   - Evidence: `codemod_cli_invocation_for_mcp` prefers `current_exe()` when it exists.

2. **Default harness is easy to get wrong**
   - Empty directory + `--no-interactive` installs Claude.
   - Re-running with `--harness cursor` then leaves both `.claude` and `.cursor` trees plus both MCP configs.
   - Interactive prompt label is “Choose harness adapter” with bare names (`claude`, `cursor`, …) and no one-line “what this installs.”
   - Auto detection prefers marker folders; absent markers, Claude wins. Cursor-first users can silently set up the wrong agent.

3. **Install logs hide warnings**
   - `print_install_output_logs` never prints `output.warnings`.
   - Table/list formats do. First-run default (`--format logs`) can hide overwrite, Goose scope, update-policy, or discovery-guide failures.

4. **Post-install does not complete the loop**
   - Restart / “enable MCP” hint is correct but incomplete.
   - No harness-specific steps (Cursor: Settings → MCP; Claude: restart session; Goose: Summon extension is buried in the same paragraph).
   - No suggested first prompt, no `ai list` / doctor verification, no link to quickstart.

### P1 — friction that makes the product feel unfinished

5. **Reinstall UX is opaque**
   - Second install without `--force` still prints full “Installed …” success.
   - No “already up to date” vs “updated” vs “skipped conflicting file” distinction.
   - Users cannot tell whether anything changed.

6. **Periodic update hooks install with little explanation**
   - Cursor gets `.cursor/hooks.json` → local `check-updates.sh`.
   - Claude gets `SessionStart` in `.claude/settings.json`.
   - Paths are workspace-absolute. Moving the repo or checking out elsewhere leaves stale hook commands.
   - First-run never explains auto-safe updates or how to choose `manual` / `notify`.

7. **Discovery guides are agent-facing, not human-facing**
   - `AGENTS.md` / `CLAUDE.md` blocks are useful for agents, dense for people.
   - Install mutates repo docs without a preview or “files that will change” summary.

8. **Command IA is overloaded**
   - Bare `codemod ai` = install.
   - Same namespace holds dump-ast / docs / tools / call / resources / feedback.
   - Help says “Install and manage Codemod AI integrations,” which understates the agent-tool hub and overstates “management.”
   - First-time humans scanning `--help` do not get a “start here” story.

9. **Docs discovery is brittle**
   - `ai docs` list uses long resource ids (`codemod-creation-workflow-instructions`).
   - Short aliases mostly work (`codemod-cli`), but phrase search like `creation workflow` returned no matches.
   - Output is TSV-ish (`name\turi\tdescription`) — fine for agents, poor for humans.

10. **`ai tools` dumps deprecated aliases**
    - First glance shows many `get_*` compatibility tools before the real operational tools.
    - Humans/agents both get a noisy catalog.

11. **Feedback consent is easy to miss**
    - `ai feedback` records consent and submits in one shot.
    - No plain-language “what we store / what we never send” summary in the success line.
    - `--allow-feedback` on install is easy to overlook.

12. **Validate-call success signaling**
    - `ai call validate_codemod_package` returns a rich JSON issue list with exit `0` even when the package is invalid.
    - Fine for MCP-shaped tooling; confusing for humans copy-pasting from docs.

13. **Bare `npx codemod` non-interactive path**
    - ASCII art on stdout, next steps on stderr, exit `1`.
    - Works, but art + split streams feel noisy in CI logs and copied terminals.

### P2 — polish / expansion

14. **Antigravity exists in adapters but not in `--harness` help values** shown by published CLI (`auto|claude|goose|opencode|cursor|codex`).
15. **Scope defaults differ by harness** (Goose pushes user; others project) without an upfront explanation of team vs personal install.
16. **npm project config warnings** when invoked inside this monorepo (`auto-install-peers`, `public-hoist-pattern`) distract from the install summary.
17. **No `codemod ai doctor` / status** command that answers: harness detected, MCP reachable, skill version, hooks present, signed-manifest policy.
18. **Quickstart stops at install** — does not show the “reload agent → run `/codemod` or ask X → verify MCP tools” moment of delight.

## Target first-run experience

A first-time user should be able to do this in under two minutes:

1. Run `npx codemod ai`.
2. Confirm harness + scope in plain language (or accept a clearly stated default).
3. See exactly which files will be written/updated before apply (or immediately after, as a crisp inventory).
4. Get durable MCP config (`npx`/`codemod` on `PATH`, not a cache path), unless they opt into “pin local binary.”
5. Finish with: restart steps for *their* harness, a copy-paste first prompt, and a one-command verify (`codemod ai doctor` or `codemod ai list`).

Success criteria:

- MCP still works after clearing the npm cache.
- Wrong-harness installs are hard; recovery is one command.
- Warnings never silent in default output.
- Re-running install is idempotent and honest (“already installed”).
- A new user knows what to type next without opening docs.

## Improvement plan

### Phase 1 — fix correctness and honesty

| Item | Change | Why |
| --- | --- | --- |
| 1.1 | Prefer portable MCP invocation (`npx codemod@latest mcp` or `codemod mcp` on PATH). Keep absolute `current_exe` only behind an explicit flag or as a fallback when `npx` is unavailable. Align docs + installer. | Stops the #1 post-install breakage. |
| 1.2 | Print warnings in default logs format (and JSON/table already have them). | Surfacing failures users currently miss. |
| 1.3 | Make reinstall idempotent messaging: `already installed`, `updated`, `skipped (needs --force)`. | Removes false “everything is new” success. |
| 1.4 | When auto-detecting harness, print the reason (`found .cursor`, or `no markers; defaulted to claude — pass --harness`). In interactive mode, lead with detected suggestion and human labels. | Prevents silent wrong-agent setup. |
| 1.5 | Rewrite restart hint into harness-specific checklists (Cursor MCP toggle, Claude reload, Goose Summon, Codex restart). | Turns a vague nudge into actionable next steps. |

### Phase 2 — exceptional onboarding

| Item | Change | Why |
| --- | --- | --- |
| 2.1 | Add a short preflight banner before prompts: what will be installed (skill, MCP, `/codemod`, discovery guide, update hook) and where. | Sets expectations; reduces surprise file writes. |
| 2.2 | Rename prompt copy: “Which coding agent?” / “Install for this repo or your user profile?” with one-line consequences. Drop “adapter.” | Less jargon, faster choice. |
| 2.3 | End with a “First prompt” block + link to quickstart. Example: `Use /codemod to migrate …` or `Search the registry for …`. | Connects setup to value. |
| 2.4 | Add `codemod ai doctor` (or `ai status`): harness, scope, skill version, MCP config command, hook path, last update check. | Gives a single recovery/verify command. |
| 2.5 | Interactive consent for periodic update policy on first install (default can stay auto-safe, but say so). | Hooks should not feel sneaky. |
| 2.6 | Prefer repo-relative or `$VAR`-stable hook commands where harnesses allow; document absolute-path limitations. | Repo moves should not orphan update checks. |

### Phase 3 — command surface cleanup

| Item | Change | Why |
| --- | --- | --- |
| 3.1 | Split mental model in help/docs: `codemod ai` = setup; `codemod ai <tool>` = agent/CLI toolkit. Consider `codemod ai setup` alias for install. | Clearer IA. |
| 3.2 | Humanize `ai docs` listing (grouped, short names, examples). Improve search tokenization / fuzzy match. | Docs become usable without MCP. |
| 3.3 | Hide deprecated tool aliases behind `--all` on `ai tools`. | First glance shows the four real tools. |
| 3.4 | Feedback: show consent summary + categories cheat-sheet; keep message length guidance visible. | Trust + better signal. |
| 3.5 | Optionally non-zero exit for `validate_codemod_package` when severity=error and stdout is a TTY / `--fail` flag. | Human copy-paste flows get a clear fail. |

### Phase 4 — docs and packaging

| Item | Change | Why |
| --- | --- | --- |
| 4.1 | Update Quickstart + MCP pages with the real post-install checklist and portable MCP snippet that matches installer output. | Docs ↔ product parity. |
| 4.2 | Add a “verify setup” snippet: `npx codemod ai doctor` / `list` + “ask your agent to call dump_ast.” | Closes the loop in docs. |
| 4.3 | npm README quick start should lead with `npx codemod ai` for AI-native setup, then init/run. | Matches how people enter now. |
| 4.4 | Reduce noise when run inside repos with custom `.npmrc` if feasible (or document `npm_config_*` workarounds). | Cleaner first impression. |

## Suggested implementation order

1. Portable MCP config + warning visibility + honest reinstall messages.
2. Harness detection messaging + harness-specific next steps.
3. `ai doctor` + first-prompt footer.
4. Help/IA cleanup, docs search, tools list filtering.
5. Docs/README parity pass.

## Test plan for the UX bar

- Empty temp dir, Cursor-only machine markers, Claude-only, no markers.
- Install → delete npm cache → open harness MCP → tool still starts.
- Install twice without `--force`; third time with conflicting hand-edited MCP entry.
- `--format logs` with injected warning still shows it.
- Goose project scope shows the `/codemod` limitation prominently.
- Interactive copy review on a real TTY for each harness.
- Quickstart followed by a cold agent session: `/codemod` or skill discovery works without tribal knowledge.

## Out of scope for this plan

- Changing MCS skill authoring policy itself.
- Hosted/enterprise MCP account flows.
- Full redesign of bare `npx codemod` launcher beyond first-run adjacency.

## Dogfood appendix (commands exercised)

```bash
npx codemod@latest --help
npx codemod@latest ai --help
npx codemod@latest ai --no-interactive --harness cursor --project
npx codemod@latest ai --no-interactive                 # empty dir → claude
npx codemod@latest ai list --harness cursor|claude|auto
npx codemod@latest ai docs
npx codemod@latest ai docs codemod-cli
npx codemod@latest ai docs "creation workflow"         # no matches
npx codemod@latest ai tools
npx codemod@latest ai dump-ast --
npx codemod@latest ai node-types badlang               # exit 1
npx codemod@latest ai call validate_codemod_package --input '{"package_path":"."}'
npx codemod@latest ai feedback --category cli --message "..."
npx codemod@latest ai update --harness cursor --project --no-interactive
```

Observed MCP snippet after install (problem):

```json
{
  "mcpServers": {
    "codemod": {
      "command": "/home/ubuntu/.npm/_npx/.../codemod",
      "args": ["mcp"]
    }
  }
}
```

Documented / expected portable snippet:

```json
{
  "mcpServers": {
    "codemod": {
      "command": "npx",
      "args": ["codemod@latest", "mcp"]
    }
  }
}
```

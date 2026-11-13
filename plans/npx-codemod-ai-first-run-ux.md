# `npx codemod ai` first-run UX plan

Dogfood of published `codemod@latest`, scored against the Codex CLI UX design principles. Scope for this plan is **`npx codemod ai` onboarding and agent setup**. Broader `codemod run` / workflow execution UX is noted as a sibling backlog, not mixed into Phase 1.

## North star

Make complex codebase operations feel effortless, safe, fast, and trustworthy for both developers and AI agents.

- **Codemod CLI:** the most reliable way to discover, execute, and validate code transformations.
- **`npx codemod ai`:** the easiest way to equip coding agents with Codemod’s capabilities.

Success = less friction + less uncertainty + fewer mistakes + faster outcomes.

For this plan’s slice: a developer or agent should go from zero → working Codemod skill/MCP/`/codemod` in under two minutes, with portable config, honest output, and one obvious next step.

## Design principles (actionable for `codemod ai`)

| # | Principle | Requirement for first-run AI setup |
| --- | --- | --- |
| 1 | Immediate value | One command; minimal config; first useful agent action suggested immediately. |
| 2 | Progressive disclosure | Simple harness/scope defaults first; update policy, force, formats on demand. |
| 3 | Predictability | Same flags/output across harnesses; idempotent reinstall messaging. |
| 4 | Transparency | Say what will be written, why harness/scope was chosen, and what’s next. |
| 5 | Safety & trust | No silent overwrites; hooks/MCP side effects explicit; safe reinstall/upgrade/uninstall path. |
| 6 | Actionable errors | Failures name impact + exact recovery command. |
| 7 | Low cognitive load | Auto-detect harness from markers; don’t ask what can be inferred safely. |
| 8 | Performance | Install acknowledges quickly; no unexplained hangs. |
| 9 | Discoverability | Help examples, first prompt, doctor/verify, docs that match installer. |
| 10 | Composability | Humans get logs; agents/CI get `--json` / stable schemas / exit codes. |
| 11 | Accessibility | TTY vs non-interactive; `NO_COLOR`; narrow terminals; no essential info only in color/emoji. |
| 12 | Clear outcomes | Success is unambiguous; verification proves skill/MCP are reachable, not only that files exist. |

### Five non-negotiables

1. **Zero unnecessary friction** — don’t ask what can safely be inferred.
2. **Zero mystery** — always communicate progress and outcomes.
3. **Zero silent surprises** — make changes, scope, and uncertainty explicit.
4. **Zero dead ends** — every failure leads toward recovery.
5. **One obvious next step** — always help the developer or agent move forward.

### UX review checklist (gate every AI-setup PR)

1. Friction: can we eliminate a step, prompt, or config?
2. Clarity: does the user know what will happen and what is happening?
3. Trust: are scope, permissions, side effects, and uncertainty explicit?
4. Recovery: can they recover from errors or interruptions safely?
5. Efficiency: fast for humans and agents?
6. Completion: is the outcome clear and verifiable?
7. Next step: is the next useful action obvious?

## Verdict

Install mostly works, but the first run still fails several non-negotiables: silent harness defaults (mystery), absolute MCP paths (trust/reliability), suppressed warnings (mystery), weak verify/next-step (completion), and opaque reinstall (predictability). The command surface also mixes “set me up” with “agent toolchain” without progressive disclosure.

## What already satisfies the principles

| Principle | Evidence today |
| --- | --- |
| Immediate value | Docs + bare-CLI next steps point at `npx codemod ai`. |
| Composability | `--format json\|yaml\|table\|logs`, `--no-interactive`, CLI fallbacks without MCP. |
| Partial clear outcomes | Success block lists skill, MCP, `/codemod`, discovery hints. |
| Agent tooling | `docs`, `dump-ast`, `node-types`, `tools`, `call`, `list` work as MCP equivalents. |

## Dogfood gaps × principles

### P0 — trust, reliability, first-run success

| Gap | Principles broken | Evidence |
| --- | --- | --- |
| **MCP pins disposable absolute binary** under `~/.npm/_npx/...` instead of documented `npx codemod@latest mcp` | 1, 3, 5, 12 | `codemod_cli_invocation_for_mcp` prefers `current_exe()`; cache cleanup breaks MCP |
| **Default/auto harness easy to get wrong** (empty dir → Claude; Cursor re-run can leave both trees) | 3, 4, 7 | Interactive label “Choose harness adapter”; auto prefers markers, else Claude |
| **Install logs hide `warnings`** | 4, 6, 12 | `print_install_output_logs` never prints warnings; table/list do |
| **Post-install doesn’t verify or guide** | 1, 9, 12 | Restart/enable MCP only; no harness checklist, doctor, or first prompt |

### P1 — predictability, safety, progressive disclosure

| Gap | Principles broken | Evidence |
| --- | --- | --- |
| **Reinstall always looks brand new** | 3, 12 | Second install still prints full “Installed …” |
| **Update hooks install with little explanation** | 4, 5 | Absolute hook paths in `.cursor/hooks.json` / Claude `SessionStart` |
| **Discovery guides mutate without preview** | 4, 5 | `AGENTS.md` / `CLAUDE.md` upserts are agent-dense |
| **Command IA overloaded** | 2, 9 | Bare `ai` = install; same namespace hosts toolkit |
| **Docs search / tools catalog noisy** | 9, 10 | Phrase search misses; deprecated `get_*` aliases dominate `ai tools` |
| **Feedback consent easy to miss** | 4, 5 | Consent + submit in one shot |
| **Validate call exits 0 with errors** | 6, 10, 12 | Rich JSON issues, success exit for humans |
| **No uninstall / clean recovery path** | 5, 6 | Safe reinstall called out by Codex; not present today |

### P2 — polish

- Antigravity adapter not in published `--harness` help values.
- Scope default differs by harness without upfront explanation.
- npm `.npmrc` noise inside this monorepo.
- Quickstart stops at install (no verify → first prompt delight).

## Journey requirements (AI setup)

### A. Installation & onboarding

- [ ] One-command install with minimal configuration.
- [ ] Auto-detect coding agent / repo markers; explain the choice.
- [ ] Clearly explain what will be installed or modified (skill, MCP, `/codemod`, discovery guide, update hook).
- [ ] Avoid auth until necessary (already true for AI setup).
- [ ] Verify successful installation **and** agent integration (`ai doctor` / MCP probe).
- [ ] Suggest one immediately useful action (first prompt / `/codemod`).
- [ ] Support safe reinstall, upgrades, and uninstall.

**Goal:** first meaningful result in under 2 minutes.

### B. Command discovery & interaction (AI namespace)

- [ ] Clear hierarchy: `codemod ai` / `ai setup` = install; subcommands = toolkit.
- [ ] `--help` with realistic examples and a “start here” story.
- [ ] Intelligent defaults from project context; interactive for beginners, flags for experts.
- [ ] Context-aware suggestions without excessive prompts.

### AI-native requirements

| Dimension | Requirement | Current gap |
| --- | --- | --- |
| Tool discovery | Concise, machine-readable capability descriptions | Deprecated aliases clutter `ai tools` |
| Structured output | Stable JSON schemas, error codes, result metadata | Validate/call success signaling weak for humans |
| Agent efficiency | Minimize tool calls, latency, token load | Docs dumps are large; search unreliable |
| Verification | Confirm installed skills and MCP tools are reachable by the agent | Files written ≠ tools available |
| Determinism | Prefer reproducible, idempotent ops | Reinstall messaging lies; MCP path non-portable |
| Permissions | Separate read-only vs write/destructive | Force overwrite / hooks not framed as side effects |
| Observability | Record outcomes and useful diagnostics | Warnings suppressed in default logs |
| Context efficiency | Return relevant results without flooding context | Resource names long; search noisy |

**Philosophy:** let agents reason about *what* needs to change; Codemod reliably handles *where*, *how*, and *at what scale*. For setup: Codemod must make the agent capable, then get out of the way.

### Sibling backlog (not this plan’s Phase 1)

Codex journeys **C–F** (execution progress stages, dry-run diffs, rollback, scanned/matched/changed summaries) belong to **`codemod run` / workflow** UX. Track separately so AI first-run stays focused.

## Target first-run experience

1. Run `npx codemod ai`.
2. Confirm harness + scope in plain language (or accept a clearly stated default with reason).
3. See files that will be written/updated.
4. Get durable MCP config (`npx`/`codemod` on `PATH`), unless explicitly pinning a local binary.
5. Finish with: harness-specific restart steps, copy-paste first prompt, one-command verify.

## Improvement plan

### Phase 1 — P0 trust & first-run success

| Item | Change | Principles |
| --- | --- | --- |
| 1.1 | Prefer portable MCP (`npx codemod@latest mcp` or `codemod mcp` on PATH). Absolute `current_exe` only via explicit flag or `npx`-unavailable fallback. Align docs. | 1, 3, 5, 12 |
| 1.2 | Print warnings in default logs format. | 4, 6, 12 |
| 1.3 | Idempotent reinstall messaging: `already installed` / `updated` / `skipped (needs --force)`. | 3, 12 |
| 1.4 | Print harness detection reason; interactive labels: “Which coding agent?” with one-line consequences. | 3, 4, 7 |
| 1.5 | Harness-specific next-step checklists (Cursor MCP toggle, Claude reload, Goose Summon, Codex restart). | 6, 9, 12 |

### Phase 2 — onboarding completion & verification

| Item | Change | Principles |
| --- | --- | --- |
| 2.1 | Preflight banner: components + paths before write. | 4, 5 |
| 2.2 | First-prompt footer + quickstart link. | 1, 9, 12 |
| 2.3 | `codemod ai doctor`: harness, scope, skill version, MCP command, hook path, reachability probe. | 9, 12 + AI verification |
| 2.4 | Explicit update-policy consent on first install (default may stay auto-safe). | 4, 5 |
| 2.5 | Stabilize hook commands (repo-relative / documented limits). | 3, 5 |
| 2.6 | `codemod ai uninstall` (or documented cleanup) for skills/MCP/hooks/discovery blocks. | 5, 6 |

### Phase 3 — IA, agents, progressive disclosure

| Item | Change | Principles |
| --- | --- | --- |
| 3.1 | `codemod ai setup` alias; help splits setup vs toolkit. | 2, 9 |
| 3.2 | Humanize `ai docs`; better search tokenization. | 9, 10 |
| 3.3 | Hide deprecated tool aliases behind `--all`. | 2, 9, 10 |
| 3.4 | Feedback consent summary + category cheat-sheet. | 4, 5 |
| 3.5 | `--fail` / TTY non-zero exit when validate reports errors. | 6, 10, 12 |

### Phase 4 — docs & packaging parity

| Item | Change | Principles |
| --- | --- | --- |
| 4.1 | Quickstart + MCP pages match portable installer output + verify checklist. | 3, 9 |
| 4.2 | “Verify setup” snippet: doctor/list + ask agent to call `dump_ast`. | 1, 12 |
| 4.3 | npm README leads with `npx codemod ai`, then init/run. | 1, 9 |
| 4.4 | Reduce or document npmrc noise when run inside custom repos. | 8, 11 |

## Implementation order

1. Portable MCP + warning visibility + honest reinstall.
2. Harness detection messaging + harness-specific next steps.
3. `ai doctor` + first-prompt footer + uninstall/cleanup.
4. Help/IA cleanup, docs search, tools filtering.
5. Docs/README parity.

## Success metrics (starting targets)

| Metric | Goal | How we’ll know |
| --- | --- | --- |
| Time to first value | &lt; 2 minutes on compatible setups | Dogfood stopwatch; later product telemetry |
| First-run success rate | 95%+ | Install + doctor green; MCP tool visible in harness |
| Agent invocation success | Maximize skill/MCP usable after install | Cold agent session can run `/codemod` or `dump_ast` |
| Error recovery time | Minutes, not archaeology | Every P0 failure has a one-line recovery command |
| User intervention rate | Minimize unnecessary prompts | Auto-detect path needs no questions when markers exist |
| Reinstall honesty | 100% accurate status lines | Second run says already installed unless changed |

Refine targets once install/doctor telemetry exists. Don’t block Phase 1 on dashboards.

## Test plan

- Empty temp dir; Cursor-only markers; Claude-only; no markers.
- Install → delete npm cache → harness MCP still starts.
- Install twice without `--force`; conflicting hand-edited MCP needs `--force` and says so.
- `--format logs` surfaces injected warnings.
- Goose project scope shows `/codemod` limitation.
- Interactive copy on a real TTY per harness.
- Cold agent session after install: skill or `/codemod` works without tribal knowledge.
- `ai doctor` fails closed when MCP command is broken.

## Out of scope (this plan)

- MCS skill authoring policy rewrites.
- Hosted/enterprise MCP account flows.
- Full `codemod run` / workflow execution UX (progress stages, dry-run trust UI, result ledgers) — sibling plan.
- Bare `npx codemod` launcher redesign beyond first-run adjacency.

## Dogfood appendix

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

# Repo maintenance workflows

Codemod workflows we run on this repo, with the codemod CLI itself.

| Workflow | Checks |
| --- | --- |
| [`workflows/health.ts`](workflows/health.ts) | `single-version-crates`: `codemod` links one version of each crate in `SINGLE_VERSION_CRATES` (crossterm). `latest-pins`: no `codemod@latest` in docs, CLI/MCP messages, `init` templates or the npm README. |

```bash
# from the repo root
npx codemod@2.0.0-alpha.7 workflow run -w .codemod/workflows/health.ts --target . --dry-run

# or from .codemod/
npm install
npm run health      # the run above
npm test            # tests/health.test.ts on tests/fixtures/{pass,duplicate-crate,latest-pin}
npm run validate
```

The checks only read, so `--dry-run` is just to keep the run out of the run store and skip the dirty-tree check.

## The report

Every workflow here returns one `Report` ([`workflows/lib/report.ts`](workflows/lib/report.ts)):
checks with `status` (`pass` / `warn` / `fail`), `metrics` and `findings` (`message`, `file`, `line`, `url`).
`finish(report)` prints it as one `::codemod-report::{…}` line, then throws when a check failed, so the
run exits 1 and `run_finished.error` names the failed checks.

Why a printed line: `--format jsonl` has no event with the workflow's return value, and a failed run
stores `output: null`, so the log line is the only place a failing run's report survives. The local
dashboard (`dash run`) reads it from the `run_log` event.

## What `health.ts` replaces

`single-version-crates` replaces `scripts/check-single-crossterm.sh`, which `cargo tree -p codemod`s
for crossterm versions. It reads `Cargo.lock` instead (no Rust toolchain needed), covers any crate in
the list, and names the chain that pulls in each extra version, e.g.
`crossterm@0.28.1 next to crossterm@0.29.0: codemod > ratatui > crossterm (pulled in by ratatui@0.29.0)`.

The script is still what CI and the agent docs run (`.github/workflows/ci.yml` "Check terminal
dependency graph", `AGENTS.md`, `crates/cli/AGENTS.md`, `.agents/skills/codemod-{cli-tui,rust-workspace}`).
To switch, add this job to `.github/workflows/ci.yml`, drop the step from `rust-test`, point those docs
at `npm run health` and delete the script. The job fails today: `latest-pins` has findings in `docs/`
to fix first.

```yaml
  repo-health:
    name: Repo health
    runs-on: ubuntu-latest
    timeout-minutes: 5
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 24
      - name: Health checks
        run: npx codemod@2.0.0-alpha.7 workflow run -w .codemod/workflows/health.ts --target . --dry-run --no-interactive
      - name: Workflow tests
        working-directory: .codemod
        run: npm ci && npm test
```

`latest-pins` skips Rust test modules (`#[cfg(test)]` followed by `mod`), as
`crates/cli/src/utils/cli_command.rs`'s `no_message_points_at_codemod_latest` test does, except that
test stops at the file's *first* `#[cfg(test)]`, so it skips most of `commands/harness_adapter.rs`
(whose first one is on a single function at line 47).

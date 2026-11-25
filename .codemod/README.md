# Repo maintenance workflows

Codemod workflows we run on this repo, with the codemod CLI itself.

| Workflow | Checks |
| --- | --- |
| [`workflows/health.ts`](workflows/health.ts) | `single-version-crates`: `codemod` links one version of each crate in `SINGLE_VERSION_CRATES` (crossterm). `latest-pins`: no `codemod@latest` in docs, CLI/MCP messages, `init` templates or the npm README. |

```bash
# from the repo root, with the CLI you are working on
cargo run -p codemod -- workflow run -w .codemod/workflows/health.ts --target . --no-interactive

# or with a published CLI (2.0.0-alpha.8 or later: the report needs fail())
npx codemod@2.0.0-alpha.8 workflow run -w .codemod/workflows/health.ts --target .

# or from .codemod/
npm install
npm run health      # the run above, with the CLI in package.json
npm test            # tests/health.test.ts on tests/fixtures/{pass,duplicate-crate,latest-pin}
npm run validate
```

The checks only read, so a real run is fine on a tree with uncommitted changes: the CLI checks for
them only before a run first writes.

CI runs both in `.github/workflows/maintenance.yml` ("Repo health"), with the CLI built from the same
commit (`CODEMOD_BINARY` points the tests at it).

## The report

Every workflow here returns one `Report` ([`workflows/lib/report.ts`](workflows/lib/report.ts)):
checks with `status` (`pass` / `warn` / `fail`), `metrics` and `findings` (`message`, `file`, `line`, `url`).
`finish(report)` returns it, and when a check failed it calls `fail()` from `codemod/workflow` with the
report as the output: the run ends `failed`, exits 2, `run_finished.error` names the failed checks, and
the report is still the run's output:

- `--format jsonl`: `run_finished.output`;
- `codemod workflow status --id <id> --format json`: `output`;
- tests: `runWorkflow(...).output`;
- text: the last line, `Output: {…}`.

The local dashboard (`dash run`) reads it from `run_finished.output`.

## What `health.ts` replaces

`single-version-crates` replaced `scripts/check-single-crossterm.sh`, which `cargo tree -p codemod`ed
for crossterm versions. It reads `Cargo.lock` instead (no Rust toolchain needed), covers any crate in
the list, and names the chain that pulls in each extra version, e.g.
`crossterm@0.28.1 next to crossterm@0.29.0: codemod > ratatui > crossterm (pulled in by ratatui@0.29.0)`.
`Cargo.lock` holds every target's dependencies, so it is at least as strict as `cargo tree` on one host.

`latest-pins` skips Rust test code (`#[cfg(test)]` modules), like `crates/cli/src/utils/cli_command.rs`'s
`no_message_points_at_codemod_latest` test, and lines that say `@latest` is 1.x.

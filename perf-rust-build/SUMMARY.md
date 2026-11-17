# Rust-built bundles vs esbuild-built bundles (Nov 9)

Same workflows and repos, same QuickJS host. Only the bundler changes: `orchestration-build`
(oxc split + rolldown) against the TypeScript esbuild path. 3 timed runs after a warm-up.

| Workflow / repo | esbuild wall (s) | Rust wall (s) | Changed files | Diff hash |
| --- | --- | --- | --- | --- |
| util-is / bluebird | 0.29-0.31 | 0.30-0.33 | 0 / 0 | same |
| react-19 / excalidraw | 8.48-8.77 | 8.56-8.79 | 0 / 0 | same |
| audit / excalidraw | 5.65-5.97 | 5.73-5.91 | 0 / 0 | same |
| debarrel / cal.com | 20.0-20.8 | 20.0-20.4 | 188 / 188 | same (c1125779fe8cb761) |

- Probe and taxonomy repos: same diff hashes for every workflow.
- audit's 458-line findings report is byte-identical between the two bundles.
- Every run exited 0.
- Warm bundle builds in Rust: 17-46 ms; the artifact counts match the esbuild split (1, 5, 14, 1).

Raw data: rust-build.csv and logs/.

# `codemod workflow run -w workflow.ts` (Rust CLI) vs the Node CLI (Nov 9)

Same workflows, repos and QuickJS engine; the Rust CLI builds and runs the workflow in one
process, the Node CLI spawns the host. 3 timed runs after a warm-up (cli.csv, logs/).

| Workflow / repo | Node CLI wall (s) | Rust CLI wall (s) | Peak RSS Node / Rust (MB) | Diff |
| --- | --- | --- | --- | --- |
| util-is / probe | 0.12 | 0.04 | 56 / 31 | same |
| react-19 / taxonomy | 0.73 | 0.69 | 56 / 37 | same |
| util-is / bluebird | 0.39-0.45 | 0.36-0.72 | 57 / 44-48 | same |
| react-19 / excalidraw | 8.76-8.94 | 8.51-10.1 | 102-108 / 111-118 | same |
| audit / excalidraw | 5.71-5.96 | 5.61-5.72 | 120-123 / 137 | same |
| debarrel / cal.com | 21.2-21.4 | 20.9-21.6 | 316 / 327-330 | same, 188 files |

- Small repos: about 50-100 ms faster and about 20 MB lighter (no Node process).
- Large repos: same speed within noise; about 10 MB more peak memory (the CLI binary itself).
- Every diff hash matches, every run exited 0.

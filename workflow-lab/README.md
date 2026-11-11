# Workflow v2 lab

Three real workflows on the TypeScript engine (#2411), a local dashboard over
their runs, and what they showed was missing ([PRIMITIVES.md](PRIMITIVES.md)).

| Workflow | Shape | Default target |
|---|---|---|
| `react-audit` | Fan-out: nine detectors from the jssg React/Next.js audit over every repo in a folder; a hit table per repo and rule. | `~/lab-repos/react-apps` |
| `react-19-readiness` | Report: React 19 breaking APIs and declared React versions per package of a monorepo. | `~/lab-repos/backstage-src` |
| `react-19-act-migration` | Transform and verify: branch, count, rewrite `act` imports, count again, check the diff, commit. | `~/lab-repos/backstage-src` |

## Run it

```sh
pnpm install --filter @codemod.com/orchestration   # the CLI build embeds one npm package
cargo build -p codemod
workflow-lab/setup-repos.sh                        # shallow clones into ~/lab-repos
node workflow-lab/dashboard/server.mjs             # http://127.0.0.1:4319
```

Each run is `codemod workflow run -w <workflow.ts> --target ... --history ... --events ...`,
stored under `workflow-lab/.runs/<workflow>/<run>/`. The dashboard lists runs,
shows each step's status, queue time, run time and output, starts and cancels
runs, and resumes a failed or cancelled run from its history ("Replay" on a
finished one replays every step). A workflow's results view is the `view.js`
next to it; `lab.json` holds its title and default target.

Why a Node server: it is one file with no dependencies, and it only serves the
page and starts `codemod` processes; the workflows themselves run in the
codemod binary with no Node. Once the CLI writes the run store itself
(PRIMITIVES.md #3), the page can be served by the CLI.

Without the dashboard:

```sh
target/debug/codemod workflow run -w workflow-lab/workflows/react-audit/workflow.ts \
  --target ~/lab-repos/react-apps --events /tmp/audit.events.jsonl
```

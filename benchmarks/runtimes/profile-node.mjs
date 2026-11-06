#!/usr/bin/env node
// Where `ts-node` time goes: runs ts/workflow.ts on the node runtime in this
// process with simple timers and prints a breakdown (milliseconds) to stdout.
// Mutates the repository like the arm itself; reset it afterwards.
//
// The orchestration sources need full TypeScript transforms (parameter
// properties), which `bin/register-ts.mjs` installs (as `bin/codemod-workflow.mjs` does):
//
//   node --import ./packages/orchestration/bin/register-ts.mjs \
//     benchmarks/runtimes/profile-node.mjs [repo]   # default $BENCH_REPO or ~/.cache/codemod-bench/vite
import { homedir } from "node:os";
import { resolve } from "node:path";

const startup = performance.now(); // node boot until this module runs
const repo = resolve(
  process.argv[2] ?? process.env.BENCH_REPO ?? resolve(homedir(), ".cache/codemod-bench/vite"),
);
const workflow = resolve(import.meta.dirname, "ts/workflow.ts");

let mark = performance.now();
const lap = () => {
  const now = performance.now();
  const ms = now - mark;
  mark = now;
  return ms;
};
const orchestration = await import("../../packages/orchestration/src/index.ts");
const importMs = lap();
const { executable, artifacts } = await orchestration.loadExecutable(workflow, false);
const buildMs = lap();
const phases = new Map();
const profile = (phase, ms) => phases.set(phase, (phases.get(phase) ?? 0) + ms);
const executor = new orchestration.NodeExecutor({ cwd: repo, artifacts, profile });
await orchestration.run(executable, { executor, runtime: orchestration.NODE_RUNTIME });
const runMs = lap();

const row = (name, ms) => process.stdout.write(`${name.padEnd(34)} ${ms.toFixed(1).padStart(8)}\n`);
row("node startup (boot to script)", startup);
row("import orchestration", importMs);
row("load workflow + build artifacts", buildMs);
row("run (both jssg commands)", runMs);
for (const [phase, ms] of [...phases].sort(([a], [b]) => a.localeCompare(b))) row(`  ${phase}`, ms);
const accounted = ["jssg.select", "jssg.dispatch", "jssg.stage", "jssg.commit"].reduce(
  (sum, phase) => sum + (phases.get(phase) ?? 0),
  0,
);
row("  (run - select/dispatch/stage/commit)", runMs - accounted);
row("total in process", startup + importMs + buildMs + runMs);

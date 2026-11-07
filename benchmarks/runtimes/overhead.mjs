#!/usr/bin/env node
// Summarize overhead.sh output: per arm, median wall with timings off and on,
// and median(on) / median(off) with a bootstrap 95 % CI, judged against the
// budget (default 1 %): "within" when the CI's upper end is <= 1 + budget,
// "OVER" when its lower end is above it, else "inconclusive". Also reports
// how many "on" runs actually produced a timings file. Exit status 1 when an
// arm is OVER.
//
//   node overhead.mjs <dir> [--budget 0.01] [--seed N] [--resamples N]
import { existsSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { loadHyperfineRun } from "./analysis/load.mjs";
import { bootstrapMedianRatio, median } from "./analysis/stats.mjs";

export function summarizeOverhead(dir, { budget = 0.01, seed = 1, resamples = 10000 } = {}) {
  const raw = join(dir, "raw");
  const arms = {};
  for (const name of readdirSync(raw).sort()) {
    const m = /^(\d+)-(.+)-(off|on)\.json$/.exec(name);
    if (!m || name.includes(".timings.")) continue;
    const [, , arm, mode] = m;
    const run = loadHyperfineRun(join(raw, name));
    if (run.exit !== 0) throw new Error(`${name}: exit ${run.exit}`);
    const a = (arms[arm] ??= { off: [], on: [], timings_files: 0 });
    a[mode].push(run.wall_s);
    if (mode === "on" && existsSync(join(raw, name.replace(/\.json$/, ".timings.json"))))
      a.timings_files++;
  }
  const rows = Object.entries(arms).map(([arm, a]) => {
    const r = bootstrapMedianRatio(a.on, a.off, { seed, resamples });
    const limit = 1 + budget;
    const verdict = r.hi <= limit ? "within" : r.lo > limit ? "OVER" : "inconclusive";
    return {
      arm,
      n: Math.min(a.on.length, a.off.length),
      off_median_s: median(a.off),
      on_median_s: median(a.on),
      ...r,
      verdict,
      timings_files: a.timings_files,
    };
  });
  return { budget, rows };
}

export function renderOverhead({ budget, rows }) {
  const out = [
    `| Arm | n | Off [s] | On [s] | On / off (95 % CI) | Budget ${(budget * 100).toFixed(0)} % | Timings files |`,
    "|:---|---:|---:|---:|---:|:---|---:|",
  ];
  for (const r of rows) {
    out.push(
      `| \`${r.arm}\` | ${r.n} | ${r.off_median_s.toFixed(3)} | ${r.on_median_s.toFixed(3)} | ${r.ratio.toFixed(4)} [${r.lo.toFixed(4)}, ${r.hi.toFixed(4)}] | ${r.verdict} | ${r.timings_files}/${r.n} |`,
    );
  }
  return `${out.join("\n")}\n`;
}

function main() {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      budget: { type: "string", default: "0.01" },
      seed: { type: "string", default: "1" },
      resamples: { type: "string", default: "10000" },
    },
  });
  if (positionals.length !== 1) {
    process.stderr.write("usage: overhead.mjs <dir> [--budget 0.01]\n");
    process.exit(64);
  }
  const result = summarizeOverhead(resolve(positionals[0]), {
    budget: Number(values.budget),
    seed: Number(values.seed),
    resamples: Number(values.resamples),
  });
  process.stdout.write(renderOverhead(result));
  if (result.rows.some((r) => r.verdict === "OVER")) process.exit(1);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();

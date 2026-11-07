#!/usr/bin/env node
// Summarize overhead.sh output: per arm, median wall with timings off and on,
// median(on) / median(off) with a bootstrap 95 % CI, and the median of the
// per-round on / off ratios (each round runs off and on back to back) with a
// paired bootstrap CI. The paired CI is judged against the budget (default
// 1 %): "within" when its upper end is <= 1 + budget, "OVER" when its lower
// end is above it, else "inconclusive". Also reports
// how many "on" runs actually produced a timings file. Exit status 1 when an
// arm is OVER.
//
//   node overhead.mjs <dir> [--budget 0.01] [--seed N] [--resamples N]
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { loadHyperfineRun } from "./analysis/load.mjs";
import { bootstrapMedianRatio, bootstrapPairedRatio, median } from "./analysis/stats.mjs";

export function summarizeOverhead(dir, { budget = 0.01, seed = 1, resamples = 10000 } = {}) {
  const raw = join(dir, "raw");
  const arms = {};
  for (const name of readdirSync(raw).sort()) {
    const m = /^(\d+)-(.+)-(off|on)\.json$/.exec(name);
    if (!m || name.includes(".timings.")) continue;
    const [, round, arm, mode] = m;
    const run = loadHyperfineRun(join(raw, name));
    if (run.exit !== 0) throw new Error(`${name}: exit ${run.exit}`);
    const a = (arms[arm] ??= { off: [], on: [], rounds: {}, timings_files: 0 });
    a[mode].push(run.wall_s);
    (a.rounds[round] ??= {})[mode] = run.wall_s;
    if (mode === "on" && existsSync(join(raw, name.replace(/\.json$/, ".timings.json"))))
      a.timings_files++;
  }
  const limit = 1 + budget;
  const judge = (r) => (r.hi <= limit ? "within" : r.lo > limit ? "OVER" : "inconclusive");
  const rows = Object.entries(arms).map(([arm, a]) => {
    const r = bootstrapMedianRatio(a.on, a.off, { seed, resamples });
    // The off and on runs of one round ran back to back: pair them.
    const pairs = Object.values(a.rounds).filter((p) => p.on !== undefined && p.off !== undefined);
    const paired = bootstrapPairedRatio(
      pairs.map((p) => p.on),
      pairs.map((p) => p.off),
      { seed, resamples },
    );
    return {
      arm,
      n: Math.min(a.on.length, a.off.length),
      off_median_s: median(a.off),
      on_median_s: median(a.on),
      ...r,
      paired: { n: pairs.length, ...paired },
      // The verdict uses the paired interval (the interleaved design's estimate).
      verdict: judge(pairs.length > 0 ? paired : r),
      timings_files: a.timings_files,
    };
  });
  return { budget, rows };
}

export function renderOverhead({ budget, rows }) {
  const out = [
    `| Arm | n | Off [s] | On [s] | median(on) / median(off) (95 % CI) | Paired on / off (95 % CI) | Budget ${(budget * 100).toFixed(0)} % | Timings files |`,
    "|:---|---:|---:|---:|---:|---:|:---|---:|",
  ];
  const ci = (r) => `${r.ratio.toFixed(4)} [${r.lo.toFixed(4)}, ${r.hi.toFixed(4)}]`;
  for (const r of rows) {
    out.push(
      `| \`${r.arm}\` | ${r.n} | ${r.off_median_s.toFixed(3)} | ${r.on_median_s.toFixed(3)} | ${ci(r)} | ${ci(r.paired)} | ${r.verdict} | ${r.timings_files}/${r.n} |`,
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
  const modeFile = join(resolve(positionals[0]), "mode");
  const mode = existsSync(modeFile) ? readFileSync(modeFile, "utf8").trim() : "env";
  process.stdout.write(
    mode === "build"
      ? "Mode: build (off = baseline build without the `timings` feature, on = default build; CODEMOD_TIMINGS_FILE unset in both)\n\n"
      : "Mode: env (one build; CODEMOD_TIMINGS_FILE unset vs set)\n\n",
  );
  process.stdout.write(renderOverhead(result));
  if (result.rows.some((r) => r.verdict === "OVER")) process.exit(1);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();

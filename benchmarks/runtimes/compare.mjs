#!/usr/bin/env node
// Compare two analyze.mjs reports (no dependencies).
//
//   node compare.mjs old/report.json new/report.json [--seed N] [--resamples N] [--threshold 0.03]
//
// For every arm and size in both reports, and every phase both have timings
// for: the change of the median (new / old - 1) with a bootstrap 95 % CI.
// A change is a REGRESSION when it exceeds the threshold (default 3 %) and
// its CI excludes 0; an improvement likewise below -threshold. Prints a
// markdown table; exit status 1 when anything regressed.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { bootstrapMedianRatio } from "./analysis/stats.mjs";

function verdict(c, threshold) {
  if (c.change > threshold && c.lo > 0) return "REGRESSION";
  if (c.change < -threshold && c.hi < 0) return "improvement";
  return "";
}

function change(newSamples, oldSamples, opts) {
  const r = bootstrapMedianRatio(newSamples, oldSamples, opts);
  return { change: r.ratio - 1, lo: r.lo - 1, hi: r.hi - 1 };
}

export function compareReports(
  oldReport,
  newReport,
  { seed = 1, resamples = 10000, threshold = 0.03 } = {},
) {
  const rows = [];
  const opts = { seed, resamples };
  for (const ns of newReport.sizes) {
    const os = oldReport.sizes.find((s) => s.size === ns.size);
    if (!os) continue;
    for (const [arm, na] of Object.entries(ns.arms)) {
      const oa = os.arms[arm];
      if (!oa) continue;
      const wall = change(na.samples_s, oa.samples_s, opts);
      rows.push({
        size: ns.size,
        arm,
        metric: "wall",
        old: oa.median_s * 1e3,
        new: na.median_s * 1e3,
        ...wall,
        verdict: verdict(wall, threshold),
      });
      if (!na.timings || !oa.timings) continue;
      for (const [phase, np] of Object.entries(na.timings.phases)) {
        const op = oa.timings.phases[phase];
        if (!op) continue;
        if (np.median_ns === 0 && op.median_ns === 0) continue;
        const c = change(np.samples_ns, op.samples_ns, opts);
        rows.push({
          size: ns.size,
          arm,
          metric: phase,
          old: op.median_ns / 1e6,
          new: np.median_ns / 1e6,
          ...c,
          verdict: Number.isFinite(c.change) ? verdict(c, threshold) : "",
        });
      }
    }
  }
  return { threshold, rows, regressions: rows.filter((r) => r.verdict === "REGRESSION") };
}

const p1 = (x) => (Number.isFinite(x) ? `${x >= 0 ? "+" : ""}${(x * 100).toFixed(1)} %` : "n/a");

export function renderComparison({ rows, threshold, regressions }) {
  const out = [
    `| Size | Arm | Metric | Old [ms] | New [ms] | Change (95 % CI) | Flag |`,
    `|:---|:---|:---|---:|---:|---:|:---|`,
  ];
  for (const r of rows) {
    out.push(
      `| ${r.size} | \`${r.arm}\` | ${r.metric} | ${r.old.toFixed(1)} | ${r.new.toFixed(1)} | ${p1(r.change)} [${p1(r.lo)}, ${p1(r.hi)}] | ${r.verdict} |`,
    );
  }
  out.push(
    "",
    regressions.length
      ? `${regressions.length} regression(s) above ${(threshold * 100).toFixed(0)} % with a CI excluding 0.`
      : `No regression above ${(threshold * 100).toFixed(0)} % with a CI excluding 0.`,
    "",
  );
  return out.join("\n");
}

function main() {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      seed: { type: "string", default: "1" },
      resamples: { type: "string", default: "10000" },
      threshold: { type: "string", default: "0.03" },
    },
  });
  if (positionals.length !== 2) {
    process.stderr.write("usage: compare.mjs old/report.json new/report.json [--threshold 0.03]\n");
    process.exit(64);
  }
  const [oldReport, newReport] = positionals.map((p) => JSON.parse(readFileSync(p, "utf8")));
  const result = compareReports(oldReport, newReport, {
    seed: Number(values.seed),
    resamples: Number(values.resamples),
    threshold: Number(values.threshold),
  });
  process.stdout.write(renderComparison(result));
  if (result.regressions.length) process.exit(1);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();

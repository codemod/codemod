#!/usr/bin/env node
// Analyze rounds.sh output (no dependencies).
//
//   node analyze.mjs <dir>... [--out <dir>] [--seed N] [--resamples N]
//
// Each <dir> is a size directory written by rounds.sh (it has meta.json) or a
// session directory holding several (sizes.sh). Writes report.md and
// report.json to --out (default: results/ beside this script).
//
// Per arm and size: median, IQR, min, max, n, and a bootstrap 95 % CI of the
// median ratio to the fastest arm. Across sizes: wall = fixed + per_file x N
// with CIs. Timings files (top-level + children) become a per-phase table.
// Exit status 2 when any arm's IQR/median exceeds 5 %, unless
// BENCH_ALLOW_NOISY=1 (the reports are written either way).
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { findSizeDirs, loadSizeDir } from "./analysis/load.mjs";
import { buildReport, renderMarkdown } from "./analysis/report.mjs";

export function analyze(paths, { out, seed = 1, resamples = 10000 } = {}) {
  const sizes = findSizeDirs(paths).map(loadSizeDir);
  const report = buildReport(sizes, { seed, resamples });
  const markdown = renderMarkdown(report, { preflight: report.provenance.preflight });
  if (out) {
    mkdirSync(out, { recursive: true });
    writeFileSync(join(out, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
    writeFileSync(join(out, "report.md"), markdown);
  }
  return { report, markdown };
}

function main() {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      out: { type: "string" },
      seed: { type: "string", default: "1" },
      resamples: { type: "string", default: "10000" },
    },
  });
  if (positionals.length === 0) {
    process.stderr.write("usage: analyze.mjs <dir>... [--out <dir>] [--seed N] [--resamples N]\n");
    process.exit(64);
  }
  const out = resolve(values.out ?? join(import.meta.dirname, "results"));
  const { report, markdown } = analyze(positionals, {
    out,
    seed: Number(values.seed),
    resamples: Number(values.resamples),
  });
  process.stdout.write(markdown);
  process.stderr.write(`[bench] report: ${join(out, "report.md")}, ${join(out, "report.json")}\n`);
  if (!report.noise.ok) {
    const list = report.noise.noisy
      .map((n) => `${n.size}/${n.arm} ${(n.iqr_rel * 100).toFixed(1)} %`)
      .join(", ");
    if (process.env.BENCH_ALLOW_NOISY === "1") {
      process.stderr.write(`[bench] noisy (allowed by BENCH_ALLOW_NOISY=1): ${list}\n`);
    } else {
      process.stderr.write(
        `[bench] noise check FAILED (IQR/median > 5 %): ${list}; BENCH_ALLOW_NOISY=1 to accept\n`,
      );
      process.exit(2);
    }
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();

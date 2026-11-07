// Tests for analyze.mjs / compare.mjs / overhead.mjs on the timings fixtures
// (fixtures/timings, one file per arm plus child files for the bridge and
// quickjs-host processes) and synthetic hyperfine exports. Run with
// `node --test benchmarks/runtimes/tests/` (or `pnpm test:bench`).
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, describe, test } from "node:test";

import { analyze } from "../analyze.mjs";
import { compareReports, renderComparison } from "../compare.mjs";
import { loadTimings } from "../analysis/load.mjs";
import { asciiPlot } from "../analysis/report.mjs";
import {
  bootstrapFit,
  bootstrapMedianRatio,
  median,
  quantile,
  rng,
  summarize,
} from "../analysis/stats.mjs";
import { summarizeOverhead } from "../overhead.mjs";

const HERE = import.meta.dirname;
const FIXTURES = join(HERE, "../fixtures/timings");
const ARMS = ["yaml", "ts-bridge", "ts-node", "ts-quickjs"];
// Synthetic truth: wall = fixed + per_file * N (seconds).
const MODEL = {
  yaml: { fixed: 1.3, perFile: 0.00002 },
  "ts-bridge": { fixed: 0.07, perFile: 0.001 },
  "ts-node": { fixed: 0.045, perFile: 0.00019 },
  "ts-quickjs": { fixed: 0.045, perFile: 0.00038 },
};
const SIZES = [
  { size: "probe", files: 2 },
  { size: "vite", files: 1700 },
  { size: "vite-x4", files: 6800 },
];

const tmp = mkdtempSync(join(tmpdir(), "bench-analysis-test-"));
after(() => execFileSync("rm", ["-rf", tmp]));

const hyperfineExport = (name, t) => ({
  results: [
    {
      command: name,
      mean: t,
      stddev: null,
      median: t,
      user: t * 0.8,
      system: t * 0.2,
      min: t,
      max: t,
      times: [t],
      exit_codes: [0],
    },
  ],
});

/**
 * Write a rounds.sh-shaped session: <dir>/<size>/{meta.json,raw/...}. Each
 * run's wall time is MODEL * scale(arm) times a small seeded jitter
 * (±`jitter`); timings files are the fixtures, children renamed the way the
 * processes name them (`<top>.<pid>.json`) while the top-level `children`
 * list keeps the fixtures' original absolute paths (exercising the fallback).
 */
function writeSession(
  dir,
  { rounds = 5, jitter = 0.01, seed = 7, scale = {}, timings = true, sizes = SIZES } = {},
) {
  const rand = rng(seed);
  for (const { size, files } of sizes) {
    const raw = join(dir, size, "raw");
    mkdirSync(raw, { recursive: true });
    writeFileSync(
      join(dir, size, "meta.json"),
      JSON.stringify({
        size,
        files,
        rounds,
        cache: "warm",
        purge: false,
        seed: 1,
        gate_hash: "8f7ff40f81b3",
        gate_files: 331,
        recorded_at: "2026-11-07T00:00:00.000Z",
        codemod: { sha: "abc123", branch: "rt/f-harness", dirty: false },
        input: { tag: "vite v8.3.6", sha: "0fd9a29f" },
        versions: { node: "v24.18.0", hyperfine: "hyperfine 1.20.0" },
        binaries: [{ name: "codemod", path: "/x/codemod", sha256: "deadbeef" }],
        arms: ARMS.map((arm) => ({ arm, command: `bench-exec ${arm}` })),
        preflight: "PASS  power: AC\n",
      }),
    );
    for (let round = 1; round <= rounds; round++) {
      for (const arm of ARMS) {
        const m = MODEL[arm];
        const t =
          (m.fixed + m.perFile * files) * (scale[arm] ?? 1) * (1 + (rand() * 2 - 1) * jitter);
        const base = join(raw, `${round}-${arm}`);
        writeFileSync(`${base}.json`, JSON.stringify(hyperfineExport(arm, t)));
        if (!timings) continue;
        copyFileSync(join(FIXTURES, `${arm}.timings.json`), `${base}.timings.json`);
        for (const child of readdirSync(FIXTURES).filter((n) =>
          n.startsWith(`${arm}.timings.json.`),
        )) {
          copyFileSync(
            join(FIXTURES, child),
            `${base}.timings.json${child.slice(`${arm}.timings.json`.length)}`,
          );
        }
      }
    }
  }
  return dir;
}

describe("stats", () => {
  test("quantiles use linear interpolation (type 7)", () => {
    assert.equal(median([5, 1, 3]), 3);
    assert.equal(median([1, 2, 3, 4]), 2.5);
    assert.equal(quantile([1, 2, 3, 4, 5], 0.25), 2);
    const s = summarize([1, 2, 3, 4, 5]);
    assert.deepEqual([s.n, s.median, s.q1, s.q3, s.iqr, s.min, s.max], [5, 3, 2, 4, 2, 1, 5]);
    assert.equal(s.iqr_rel, 2 / 3);
  });

  test("rng is deterministic per seed", () => {
    const a = rng(42);
    const b = rng(42);
    const xs = Array.from({ length: 5 }, () => a());
    assert.deepEqual(
      xs,
      Array.from({ length: 5 }, () => b()),
    );
    assert.ok(xs.every((x) => x >= 0 && x < 1));
    assert.notDeepEqual(xs, Array.from({ length: 5 }, rng(43)));
  });

  test("bootstrap median ratio: seeded, CI brackets the point estimate", () => {
    const a = [2.0, 2.1, 1.9, 2.05, 1.95, 2.02];
    const b = [1.0, 1.02, 0.98, 1.01, 0.99, 1.0];
    const r1 = bootstrapMedianRatio(a, b, { seed: 3, resamples: 2000 });
    const r2 = bootstrapMedianRatio(a, b, { seed: 3, resamples: 2000 });
    assert.deepEqual(r1, r2);
    assert.ok(r1.lo <= r1.ratio && r1.ratio <= r1.hi, JSON.stringify(r1));
    assert.ok(r1.lo > 1.8 && r1.hi < 2.2, JSON.stringify(r1));
    assert.deepEqual(bootstrapMedianRatio(a, a), { ratio: 1, lo: 1, hi: 1 });
  });

  test("bootstrapFit recovers fixed + slope; needs two distinct x", () => {
    const rand = rng(5);
    const groups = [10, 1000, 4000].map((x) => ({
      x,
      ys: Array.from({ length: 8 }, () => 0.5 + 0.001 * x + (rand() - 0.5) * 0.01),
    }));
    const fit = bootstrapFit(groups, { seed: 1, resamples: 2000 });
    assert.ok(Math.abs(fit.fixed.est - 0.5) < 0.01, JSON.stringify(fit.fixed));
    assert.ok(Math.abs(fit.per_unit.est - 0.001) < 1e-5, JSON.stringify(fit.per_unit));
    assert.ok(fit.fixed.lo <= 0.5 && 0.5 <= fit.fixed.hi);
    assert.ok(fit.r2 > 0.999);
    assert.equal(bootstrapFit([{ x: 1, ys: [1, 2] }]), null);
  });
});

describe("timings loading", () => {
  test("merges top-level and child processes (bridge)", () => {
    const t = loadTimings(join(FIXTURES, "ts-bridge.timings.json"));
    assert.equal(t.children.length, 2);
    assert.deepEqual(t.missing, []);
    const m = t.merged;
    assert.equal(m.processes.length, 3);
    assert.equal(m.phases.parse.ns, (210 + 215) * 1e6);
    assert.equal(m.phases.parse.count, 3400);
    assert.equal(m.phases.select.ns, 170e6); // parent only
    assert.equal(m.exec_to_main_ns, 1.5e6);
    assert.equal(m.wall_ns, 1700e6);
    assert.ok(Math.abs(m.coverage - 1636 / 1700) < 1e-9, String(m.coverage));
  });

  test("quickjs-host child; cpu_ns sums; yaml marks", () => {
    const q = loadTimings(join(FIXTURES, "ts-quickjs.timings.json")).merged;
    assert.deepEqual(
      q.processes.map((p) => p.arm),
      ["ts-quickjs", "quickjs-host"],
    );
    assert.equal(q.phases.boot.ns, (25 + 4) * 1e6);
    assert.equal(q.phases.read.cpu_ns, 140e6);
    const y = loadTimings(join(FIXTURES, "yaml.timings.json")).merged;
    assert.equal(y.marks.poll_saw - y.marks.engine_done, 1000e6);
    assert.equal(y.phases.idle_wait.ns, 1000e6);
    assert.equal(loadTimings(join(FIXTURES, "nope.json")), null);
  });

  test("every fixture follows the contract", () => {
    const phases = new Set([
      "boot",
      "plan",
      "select",
      "read",
      "setup",
      "parse",
      "transform",
      "convert",
      "conflict",
      "commit",
      "idle_wait",
    ]);
    for (const name of readdirSync(FIXTURES)) {
      const d = JSON.parse(readFileSync(join(FIXTURES, name), "utf8"));
      assert.equal(d.schema, 1, name);
      assert.equal(typeof d.pid, "number", name);
      assert.ok(Number.isInteger(d.wall_ns), name);
      assert.ok(Array.isArray(d.children), name);
      for (const [p, v] of Object.entries(d.phases)) {
        assert.ok(phases.has(p), `${name}: phase ${p}`);
        assert.ok(Number.isInteger(v.ns) && Number.isInteger(v.count), `${name}: ${p}`);
      }
      for (const mark of Object.keys(d.marks))
        assert.ok(["engine_done", "poll_saw"].includes(mark), mark);
    }
  });
});

describe("analyze", () => {
  const session = writeSession(join(tmp, "base"));
  const opts = { seed: 1, resamples: 2000 };

  test("per-size wall stats, fastest arm, ratios", () => {
    const { report } = analyze([session], opts);
    assert.deepEqual(
      report.sizes.map((s) => s.size),
      ["probe", "vite", "vite-x4"],
    );
    const vite = report.sizes[1];
    assert.equal(vite.fastest, "ts-node");
    assert.equal(vite.arms["ts-node"].n, 5);
    assert.deepEqual(vite.arms["ts-node"].vs_fastest, { ratio: 1, lo: 1, hi: 1 });
    const bridge = vite.arms["ts-bridge"];
    const truth = (0.07 + 1.7) / (0.045 + 0.00019 * 1700);
    assert.ok(
      bridge.vs_fastest.lo <= truth * 1.03 && truth * 0.97 <= bridge.vs_fastest.hi,
      JSON.stringify(bridge.vs_fastest),
    );
    assert.ok(bridge.min_s <= bridge.median_s && bridge.median_s <= bridge.max_s);
    assert.ok(report.noise.ok, JSON.stringify(report.noise));
  });

  test("linear fit across sizes recovers the model", () => {
    const { report } = analyze([session], opts);
    for (const arm of ARMS) {
      const f = report.fits[arm];
      const m = MODEL[arm];
      assert.ok(
        Math.abs(f.per_file_ms.est - m.perFile * 1e3) / (m.perFile * 1e3) < 0.03,
        `${arm} ${JSON.stringify(f.per_file_ms)}`,
      );
      assert.ok(
        f.fixed_s.lo - 0.02 <= m.fixed && m.fixed <= f.fixed_s.hi + 0.02,
        `${arm} ${JSON.stringify(f.fixed_s)}`,
      );
      assert.equal(f.points.length, 3);
    }
  });

  test("phase table merges children; coverage and exec_to_main", () => {
    const { report, markdown } = analyze([session], opts);
    const t = report.sizes[1].arms["ts-quickjs"].timings;
    assert.equal(t.runs_with_timings, 5);
    assert.equal(t.phases.parse.median_ns, 346e6);
    assert.equal(t.phases.parse.samples_ns.length, 5);
    assert.equal(t.exec_to_main_median_ns, 1.3e6);
    assert.equal(t.processes_median, 2);
    assert.equal(t.missing_children, 0);
    assert.equal(report.sizes[1].arms.yaml.timings.poll_lag_median_ns, 1000e6);
    assert.match(markdown, /## Phases: vite/);
    assert.match(markdown, /\| idle_wait \| 1000\.0 \|/);
    assert.match(markdown, /exec_to_main/);
  });

  test("deterministic for a seed; markdown has tables, plot, provenance, preflight", () => {
    const a = analyze([session], opts);
    const b = analyze([session], opts);
    assert.deepEqual(a.report, b.report);
    assert.match(a.markdown, /## Wall time: vite \(1700 files, 5 rounds, warm cache/);
    assert.match(a.markdown, /## Scaling: wall = fixed \+ per_file x N/);
    assert.match(a.markdown, /log-log; Y = yaml, B = ts-bridge, N = ts-node, Q = ts-quickjs/);
    assert.match(a.markdown, /codemod repo: abc123 on rt\/f-harness/);
    assert.match(a.markdown, /PASS {2}power: AC/);
    assert.match(a.markdown, /Noise check\n\nPassed/);
  });

  test("writes report.md and report.json", () => {
    const out = join(tmp, "out");
    analyze([join(session, "vite")], { ...opts, out });
    const json = JSON.parse(readFileSync(join(out, "report.json"), "utf8"));
    assert.equal(json.sizes.length, 1);
    assert.deepEqual(json.fits, {}); // one size: no fit
    assert.match(readFileSync(join(out, "report.md"), "utf8"), /# Runtime benchmark report/);
  });

  test("missing timings are reported, not fatal", () => {
    const dir = writeSession(join(tmp, "notimings"), { timings: false, sizes: [SIZES[1]] });
    const { report, markdown } = analyze([dir], opts);
    assert.equal(report.sizes[0].arms.yaml.timings, null);
    assert.match(markdown, /No timings files/);
  });

  test("noise check: CLI exits 2 unless BENCH_ALLOW_NOISY=1", () => {
    const dir = writeSession(join(tmp, "noisy"), { jitter: 0.3, sizes: [SIZES[1]] });
    const { report } = analyze([dir], opts);
    assert.equal(report.noise.ok, false);
    const cli = join(HERE, "../analyze.mjs");
    const run = (env) =>
      spawnSync(
        process.execPath,
        [cli, dir, "--out", join(tmp, "noisy-out"), "--resamples", "500"],
        {
          env: { ...process.env, ...env },
          encoding: "utf8",
        },
      );
    const strict = run({ BENCH_ALLOW_NOISY: "" });
    assert.equal(strict.status, 2, strict.stderr);
    assert.match(strict.stderr, /noise check FAILED/);
    assert.equal(run({ BENCH_ALLOW_NOISY: "1" }).status, 0);
  });

  test("non-zero exit codes fail the analysis", () => {
    const dir = writeSession(join(tmp, "failed"), { sizes: [SIZES[0]], rounds: 1 });
    const p = join(dir, "probe/raw/1-yaml.json");
    const doc = JSON.parse(readFileSync(p, "utf8"));
    doc.results[0].exit_codes = [142];
    writeFileSync(p, JSON.stringify(doc));
    assert.throws(() => analyze([dir], opts), /exited non-zero \(1-yaml\)/);
  });

  test("ascii plot places one marker per arm and size", () => {
    const { report } = analyze([session], opts);
    const plot = asciiPlot(report.sizes);
    const body = plot.split("\n").slice(0, 16).join("\n");
    for (const m of ["Y", "B", "N", "Q"]) assert.ok(body.includes(m) || body.includes("*"), m);
  });
});

describe("compare", () => {
  const opts = { seed: 1, resamples: 2000 };
  const base = analyze([writeSession(join(tmp, "cmp-old"), { seed: 11 })], opts).report;
  const same = analyze([writeSession(join(tmp, "cmp-same"), { seed: 12 })], opts).report;
  const slow = analyze(
    [writeSession(join(tmp, "cmp-slow"), { seed: 13, scale: { "ts-node": 1.1 } })],
    opts,
  ).report;

  test("same distribution: no regression", () => {
    const r = compareReports(base, same, opts);
    assert.equal(r.regressions.length, 0, renderComparison(r));
    const wall = r.rows.filter((x) => x.metric === "wall");
    assert.equal(wall.length, 12);
    // Identical timings fixtures: every phase change is exactly 0.
    assert.ok(r.rows.filter((x) => x.metric !== "wall").every((x) => x.change === 0));
  });

  test("10 % slower ts-node is flagged on every size, nothing else", () => {
    const r = compareReports(base, slow, opts);
    const flagged = r.regressions.map((x) => `${x.size}/${x.arm}/${x.metric}`).sort();
    assert.deepEqual(flagged, ["probe/ts-node/wall", "vite-x4/ts-node/wall", "vite/ts-node/wall"]);
    for (const x of r.regressions) assert.ok(x.change > 0.07 && x.lo > 0, JSON.stringify(x));
    assert.match(renderComparison(r), /3 regression\(s\) above 3 %/);
    // ...and the reverse direction is an improvement.
    const back = compareReports(slow, base, opts);
    assert.equal(back.regressions.length, 0);
    assert.equal(back.rows.filter((x) => x.verdict === "improvement").length, 3);
  });

  test("CLI exits 1 on a regression", () => {
    const oldP = join(tmp, "old.json");
    const newP = join(tmp, "new.json");
    writeFileSync(oldP, JSON.stringify(base));
    writeFileSync(newP, JSON.stringify(slow));
    const cli = join(HERE, "../compare.mjs");
    const r = spawnSync(process.execPath, [cli, oldP, newP, "--resamples", "500"], {
      encoding: "utf8",
    });
    assert.equal(r.status, 1, r.stderr);
    assert.match(r.stdout, /REGRESSION/);
    assert.equal(spawnSync(process.execPath, [cli, oldP, oldP, "--resamples", "500"]).status, 0);
  });
});

describe("overhead", () => {
  test("ratio with CI and verdict against the budget", () => {
    const dir = join(tmp, "overhead");
    mkdirSync(join(dir, "raw"), { recursive: true });
    const rand = rng(9);
    for (let i = 1; i <= 10; i++) {
      for (const [arm, factor] of [
        ["ts-node", 1.0],
        ["yaml", 1.08],
      ]) {
        const off = 0.4 * (1 + (rand() - 0.5) * 0.004);
        const on = 0.4 * factor * (1 + (rand() - 0.5) * 0.004);
        writeFileSync(
          join(dir, "raw", `${i}-${arm}-off.json`),
          JSON.stringify(hyperfineExport(arm, off)),
        );
        writeFileSync(
          join(dir, "raw", `${i}-${arm}-on.json`),
          JSON.stringify(hyperfineExport(arm, on)),
        );
        if (arm === "ts-node")
          copyFileSync(
            join(FIXTURES, "ts-node.timings.json"),
            join(dir, "raw", `${i}-${arm}-on.timings.json`),
          );
      }
    }
    const { rows } = summarizeOverhead(dir, { seed: 1, resamples: 2000 });
    const node = rows.find((r) => r.arm === "ts-node");
    const yaml = rows.find((r) => r.arm === "yaml");
    assert.equal(node.verdict, "within");
    assert.equal(node.timings_files, 10);
    assert.equal(yaml.verdict, "OVER");
    assert.equal(yaml.timings_files, 0);
  });
});

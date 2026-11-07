// Build report.json from loaded size directories, and render report.md.
import { PHASES } from "./load.mjs";
import { bootstrapFit, bootstrapMedianRatio, median, summarize } from "./stats.mjs";

export const NOISE_THRESHOLD = 0.05;

const ARM_ORDER = ["yaml", "ts-bridge", "ts-node", "ts-quickjs"];
const armRank = (a) => (ARM_ORDER.includes(a) ? ARM_ORDER.indexOf(a) : ARM_ORDER.length);
const byArm = (a, b) => armRank(a) - armRank(b) || a.localeCompare(b);

function medianOrNull(xs) {
  const v = xs.filter((x) => typeof x === "number" && Number.isFinite(x));
  return v.length ? median(v) : null;
}

/** Per-phase medians across runs (a run without a phase counts as 0 when the arm has timings). */
function phaseStats(runs) {
  const timed = runs.filter((r) => r.timings);
  if (timed.length === 0) return null;
  const names = new Set();
  for (const r of timed) for (const n of Object.keys(r.timings.merged.phases)) names.add(n);
  const ordered = [
    ...PHASES.filter((p) => names.has(p)),
    ...[...names].filter((p) => !PHASES.includes(p)).sort(),
  ];
  const phases = {};
  for (const name of ordered) {
    const samples = timed.map((r) => r.timings.merged.phases[name]?.ns ?? 0);
    const cpu = timed
      .map((r) => r.timings.merged.phases[name]?.cpu_ns)
      .filter((x) => x !== undefined);
    phases[name] = {
      median_ns: median(samples),
      count_median: median(timed.map((r) => r.timings.merged.phases[name]?.count ?? 0)),
      ...(cpu.length ? { cpu_median_ns: median(cpu) } : {}),
      samples_ns: samples,
    };
  }
  const marks = {};
  const markNames = new Set(timed.flatMap((r) => Object.keys(r.timings.merged.marks)));
  for (const m of markNames) marks[m] = medianOrNull(timed.map((r) => r.timings.merged.marks[m]));
  const pollLag = timed
    .map((r) => r.timings.merged.marks)
    .filter((m) => m.engine_done !== undefined && m.poll_saw !== undefined)
    .map((m) => m.poll_saw - m.engine_done);
  // Time hyperfine saw that the process did not account for: before main
  // (exec_to_main) is reported separately; this is after the exit hook.
  const outside = timed
    .filter((r) => r.timings.merged.exec_to_main_ns !== null)
    .map((r) => r.wall_s * 1e9 - r.timings.merged.exec_to_main_ns - r.timings.merged.wall_ns);
  return {
    runs_with_timings: timed.length,
    phases,
    coverage_median: medianOrNull(timed.map((r) => r.timings.merged.coverage)),
    in_process_wall_median_ns: medianOrNull(timed.map((r) => r.timings.merged.wall_ns)),
    exec_to_main_median_ns: medianOrNull(timed.map((r) => r.timings.merged.exec_to_main_ns)),
    after_exit_median_ns: medianOrNull(outside),
    marks,
    poll_lag_median_ns: medianOrNull(pollLag),
    processes_median: median(timed.map((r) => r.timings.merged.processes.length)),
    missing_children: timed.reduce((a, r) => a + r.timings.missing.length, 0),
  };
}

/**
 * sizes: [{ meta, runs }] from loadSizeDir. Options: resamples, seed,
 * noiseThreshold. The report keeps every raw sample so compare.mjs can
 * bootstrap differences between two reports.
 */
export function buildReport(
  sizes,
  { resamples = 10000, seed = 1, noiseThreshold = NOISE_THRESHOLD } = {},
) {
  const sizeReports = sizes
    .map(({ meta, runs }) => {
      const failed = runs.filter((r) => r.exit !== 0);
      if (failed.length) {
        throw new Error(
          `size ${meta.size}: ${failed.length} run(s) exited non-zero (${failed.map((r) => `${r.round}-${r.arm}`).join(", ")})`,
        );
      }
      const armNames = [...new Set(runs.map((r) => r.arm))].sort(byArm);
      const arms = {};
      for (const arm of armNames) {
        const armRuns = runs.filter((r) => r.arm === arm).sort((a, b) => a.round - b.round);
        const wall = armRuns.map((r) => r.wall_s);
        arms[arm] = {
          ...prefixed(summarize(wall), "_s", ["n", "iqr_rel"]),
          user_median_s: medianOrNull(armRuns.map((r) => r.user_s)),
          system_median_s: medianOrNull(armRuns.map((r) => r.system_s)),
          samples_s: wall,
          timings: phaseStats(armRuns),
        };
      }
      const fastest = armNames.reduce(
        (best, a) => (arms[a].median_s < arms[best].median_s ? a : best),
        armNames[0],
      );
      for (const arm of armNames) {
        arms[arm].vs_fastest = bootstrapMedianRatio(arms[arm].samples_s, arms[fastest].samples_s, {
          resamples,
          seed,
        });
      }
      return {
        size: meta.size,
        files: meta.files,
        rounds: meta.rounds,
        cache: meta.cache,
        gate: { hash: meta.gate_hash, files: meta.gate_files },
        input: meta.input ?? null,
        fastest,
        arms,
      };
    })
    .sort((a, b) => a.files - b.files);

  const fits = {};
  const allArms = [...new Set(sizeReports.flatMap((s) => Object.keys(s.arms)))].sort(byArm);
  for (const arm of allArms) {
    const groups = sizeReports
      .filter((s) => s.arms[arm])
      .map((s) => ({ x: s.files, ys: s.arms[arm].samples_s }));
    const fit = bootstrapFit(groups, { resamples, seed });
    if (fit) {
      fits[arm] = {
        fixed_s: fit.fixed,
        per_file_ms: {
          est: fit.per_unit.est * 1e3,
          lo: fit.per_unit.lo * 1e3,
          hi: fit.per_unit.hi * 1e3,
        },
        r2: fit.r2,
        points: groups.map((g) => ({ files: g.x, median_s: median(g.ys) })),
      };
    }
  }

  const noisy = [];
  for (const s of sizeReports) {
    for (const [arm, a] of Object.entries(s.arms)) {
      if (!(a.iqr_rel <= noiseThreshold)) noisy.push({ size: s.size, arm, iqr_rel: a.iqr_rel });
    }
  }
  const {
    size: _size,
    files: _files,
    rounds: _rounds,
    gate_hash: _gh,
    gate_files: _gf,
    input: _input,
    ...provenance
  } = sizes[0]?.meta ?? {};
  // Sizes analyzed together must come from the same build and settings.
  const fingerprint = (m) =>
    JSON.stringify({
      codemod: m.codemod?.sha,
      dirty: m.codemod?.dirty,
      binaries: (m.binaries ?? []).map((b) => [b.name, b.sha256]),
      cache: m.cache,
      purge: m.purge,
    });
  const warnings = sizes
    .filter(({ meta }) => fingerprint(meta) !== fingerprint(sizes[0].meta))
    .map(
      ({ meta }) =>
        `size ${meta.size} was recorded with a different build or settings than ${sizes[0].meta.size} (codemod sha, dirty flag, binary hashes, or cache mode)`,
    );
  if (provenance.codemod?.dirty_at_end && !provenance.codemod?.dirty) {
    warnings.push("the codemod worktree became dirty during the run");
  }
  return {
    schema: 1,
    analysis: { resamples, seed, noise_threshold: noiseThreshold },
    provenance,
    warnings,
    sizes: sizeReports,
    fits,
    noise: { ok: noisy.length === 0, noisy },
  };
}

function prefixed(summary, suffix, keep) {
  const out = {};
  for (const [k, v] of Object.entries(summary)) out[keep.includes(k) ? k : `${k}${suffix}`] = v;
  return out;
}

// ---------------------------------------------------------------- markdown

const s3 = (x) => (x === null || x === undefined || Number.isNaN(x) ? "n/a" : x.toFixed(3));
const ms = (ns) =>
  ns === null || ns === undefined || Number.isNaN(ns) ? "n/a" : (ns / 1e6).toFixed(1);
const pct = (x) =>
  x === null || x === undefined || Number.isNaN(x) ? "n/a" : `${(x * 100).toFixed(1)} %`;
const ci = (c, f = (x) => x.toFixed(2)) => `${f(c.est ?? c.ratio)} [${f(c.lo)}, ${f(c.hi)}]`;

function table(header, rows, align) {
  const lines = [
    `| ${header.join(" | ")} |`,
    `|${align.map((a) => (a === "r" ? "---:" : ":---")).join("|")}|`,
  ];
  for (const r of rows) lines.push(`| ${r.join(" | ")} |`);
  return lines.join("\n");
}

/** Log-log ASCII plot of median wall time against file count, one marker per arm. */
export function asciiPlot(sizeReports, { width = 60, height = 16 } = {}) {
  const arms = [...new Set(sizeReports.flatMap((s) => Object.keys(s.arms)))].sort(byArm);
  const markers = "YBNQ0123456789".split("");
  const marker = (arm) => {
    const i = ARM_ORDER.indexOf(arm);
    return i >= 0
      ? markers[i]
      : markers[4 + (arms.filter((a) => !ARM_ORDER.includes(a)).indexOf(arm) % 10)];
  };
  const pts = sizeReports.flatMap((s) =>
    Object.entries(s.arms).map(([arm, a]) => ({
      arm,
      x: Math.log10(Math.max(s.files, 1)),
      y: Math.log10(a.median_s),
    })),
  );
  if (pts.length === 0) return "";
  const [x0, x1] = [Math.min(...pts.map((p) => p.x)), Math.max(...pts.map((p) => p.x))];
  const [y0, y1] = [Math.min(...pts.map((p) => p.y)), Math.max(...pts.map((p) => p.y))];
  const col = (x) => (x1 > x0 ? Math.round(((x - x0) / (x1 - x0)) * (width - 1)) : 0);
  const row = (y) => (y1 > y0 ? height - 1 - Math.round(((y - y0) / (y1 - y0)) * (height - 1)) : 0);
  const grid = Array.from({ length: height }, () => Array(width).fill(" "));
  for (const p of pts) {
    const [r, c] = [row(p.y), col(p.x)];
    grid[r][c] = grid[r][c] === " " || grid[r][c] === marker(p.arm) ? marker(p.arm) : "*";
  }
  const label = (y) => `${(10 ** y).toPrecision(3).padStart(8)} s |`;
  const lines = grid.map((r, i) => {
    const y = y1 - (i / Math.max(height - 1, 1)) * (y1 - y0);
    return `${i === 0 || i === height - 1 || i === Math.floor(height / 2) ? label(y) : `${" ".repeat(10)} |`}${r.join("")}`;
  });
  lines.push(`${" ".repeat(11)}+${"-".repeat(width)}`);
  const xl = `${Math.round(10 ** x0)} files`;
  const xr = `${Math.round(10 ** x1)} files`;
  lines.push(
    `${" ".repeat(12)}${xl}${" ".repeat(Math.max(1, width - xl.length - xr.length))}${xr}`,
  );
  lines.push(
    `${" ".repeat(12)}(log-log; ${arms.map((a) => `${marker(a)} = ${a}`).join(", ")}; * = overlap)`,
  );
  return lines.join("\n");
}

export function renderMarkdown(report, { preflight } = {}) {
  const out = ["# Runtime benchmark report", ""];
  const p = report.provenance;
  out.push("## Provenance", "");
  const line = (k, v) => v !== undefined && v !== null && out.push(`- ${k}: ${v}`);
  line("Recorded", p.recorded_at);
  if (p.codemod)
    line(
      "codemod repo",
      `${p.codemod.sha}${p.codemod.dirty ? " (dirty)" : ""} on ${p.codemod.branch}`,
    );
  for (const s of report.sizes) {
    if (s.input) line(`Input \`${s.size}\``, `${s.input.tag ?? "?"} at ${s.input.sha ?? "?"}`);
  }
  line("Cache mode", p.cache + (p.purge ? " + purge" : ""));
  if (p.versions) {
    line(
      "Versions",
      Object.entries(p.versions)
        .map(([k, v]) => `${k} ${v}`)
        .join("; "),
    );
  }
  for (const b of p.binaries ?? [])
    line(`\`${b.name}\``, `${b.path} sha256 ${b.sha256 ?? "missing"}`);
  const repoPath = report.sizes[0]?.input?.path;
  for (const a of p.arms ?? []) {
    const command = repoPath ? a.command.replaceAll(repoPath, "$REPO") : a.command;
    line(`arm \`${a.arm}\``, `\`${command}\``);
  }
  for (const w of report.warnings ?? []) line("**Warning**", w);
  line(
    "Analysis",
    `${report.analysis.resamples} bootstrap resamples, seed ${report.analysis.seed}; noise threshold IQR/median <= ${pct(report.analysis.noise_threshold)}`,
  );
  out.push("");

  for (const s of report.sizes) {
    out.push(
      `## Wall time: ${s.size} (${s.files} files, ${s.rounds} rounds, ${s.cache} cache; gate ${s.gate.files} files, ${String(s.gate.hash).slice(0, 12)})`,
      "",
    );
    const arms = Object.keys(s.arms).sort((a, b) => s.arms[a].median_s - s.arms[b].median_s);
    out.push(
      table(
        [
          "Arm",
          "n",
          "Median [s]",
          "IQR [s]",
          "IQR/median",
          "Min [s]",
          "Max [s]",
          "User [s]",
          "System [s]",
          `vs \`${s.fastest}\` (95 % CI)`,
        ],
        arms.map((arm) => {
          const a = s.arms[arm];
          return [
            `\`${arm}\``,
            a.n,
            s3(a.median_s),
            s3(a.iqr_s),
            pct(a.iqr_rel),
            s3(a.min_s),
            s3(a.max_s),
            s3(a.user_median_s),
            s3(a.system_median_s),
            ci(a.vs_fastest),
          ];
        }),
        ["l", "r", "r", "r", "r", "r", "r", "r", "r", "r"],
      ),
      "",
    );
  }

  if (Object.keys(report.fits).length) {
    out.push("## Scaling: wall = fixed + per_file x N", "");
    out.push(
      table(
        [
          "Arm",
          "Fixed [s] (95 % CI)",
          "Per file [ms] (95 % CI)",
          "R^2",
          "Points (files: median s)",
        ],
        Object.entries(report.fits).map(([arm, f]) => [
          `\`${arm}\``,
          ci(f.fixed_s, (x) => x.toFixed(3)),
          ci(f.per_file_ms, (x) => x.toFixed(4)),
          f.r2.toFixed(4),
          f.points.map((pt) => `${pt.files}: ${pt.median_s.toFixed(3)}`).join(", "),
        ]),
        ["l", "r", "r", "r", "l"],
      ),
      "",
    );
  }
  if (report.sizes.length > 1) out.push("```text", asciiPlot(report.sizes), "```", "");

  for (const s of report.sizes) {
    const arms = Object.keys(s.arms)
      .filter((a) => s.arms[a].timings)
      .sort(byArm);
    out.push(`## Phases: ${s.size} (median ms per run, all processes merged)`, "");
    if (arms.length === 0) {
      out.push(
        "No timings files (CODEMOD_TIMINGS_FILE unsupported by these builds, or disabled).",
        "",
      );
      continue;
    }
    const names = [...new Set(arms.flatMap((a) => Object.keys(s.arms[a].timings.phases)))];
    const rows = names.map((n) => [
      n,
      ...arms.map((a) => ms(s.arms[a].timings.phases[n]?.median_ns)),
    ]);
    const t = (f) => arms.map((a) => f(s.arms[a].timings));
    rows.push(
      ["**coverage** (phases / in-process wall)", ...t((x) => pct(x.coverage_median))],
      ["exec_to_main", ...t((x) => ms(x.exec_to_main_median_ns))],
      ["in-process wall", ...t((x) => ms(x.in_process_wall_median_ns))],
      [
        "after exit hook (hyperfine − exec_to_main − in-process)",
        ...t((x) => ms(x.after_exit_median_ns)),
      ],
      ["hyperfine wall", ...arms.map((a) => ms(s.arms[a].median_s * 1e9))],
      ["poll lag (poll_saw − engine_done)", ...t((x) => ms(x.poll_lag_median_ns))],
      ["processes", ...t((x) => String(x.processes_median))],
      [
        "runs with timings",
        ...arms.map((a) => `${s.arms[a].timings.runs_with_timings}/${s.arms[a].n}`),
      ],
    );
    out.push(
      table(["Phase", ...arms.map((a) => `\`${a}\``)], rows, ["l", ...arms.map(() => "r")]),
      "",
    );
    const missing = arms.reduce((n, a) => n + s.arms[a].timings.missing_children, 0);
    if (missing) out.push(`Warning: ${missing} listed child timings file(s) were missing.`, "");
  }

  out.push("## Noise check", "");
  if (report.noise.ok) {
    out.push(`Passed: every arm has IQR/median <= ${pct(report.analysis.noise_threshold)}.`, "");
  } else {
    out.push(`FAILED: IQR/median above ${pct(report.analysis.noise_threshold)}:`, "");
    for (const n of report.noise.noisy) out.push(`- ${n.size} \`${n.arm}\`: ${pct(n.iqr_rel)}`);
    out.push("");
  }
  if (preflight) out.push("## Preflight", "", "```text", preflight.trimEnd(), "```", "");
  return out.join("\n");
}

// Loading raw rounds.sh output: hyperfine exports and timings JSON files
// (schema 1, see README "Timings"), including child-process files.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { basename, dirname, isAbsolute, join, resolve } from "node:path";

export const PHASES = [
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
];

const readJson = (path) => JSON.parse(readFileSync(path, "utf8"));

/** Size directories under the given paths: a directory with meta.json, or a session holding such directories. */
export function findSizeDirs(paths) {
  const dirs = [];
  for (const p of paths) {
    const abs = resolve(p);
    if (existsSync(join(abs, "meta.json"))) {
      dirs.push(abs);
      continue;
    }
    if (!existsSync(abs) || !statSync(abs).isDirectory()) throw new Error(`not a directory: ${p}`);
    const found = readdirSync(abs)
      .map((name) => join(abs, name))
      .filter((d) => existsSync(join(d, "meta.json")));
    if (found.length === 0) throw new Error(`no meta.json in ${p} or its subdirectories`);
    dirs.push(...found.sort());
  }
  return dirs;
}

/**
 * Resolve a child path listed in a timings file. Children are written next to
 * the top-level file as `<top>.<pid>.json`; if the listed path does not
 * exist (the directory moved), fall back to that name beside `topPath`.
 */
function resolveChild(topPath, child) {
  const listed = isAbsolute(child) ? child : join(dirname(topPath), child);
  if (existsSync(listed)) return listed;
  const pid = /\.(\d+)\.json$/.exec(child)?.[1];
  if (pid) {
    const beside = `${topPath}.${pid}.json`;
    if (existsSync(beside)) return beside;
  }
  const byName = join(dirname(topPath), basename(child));
  return existsSync(byName) ? byName : null;
}

/**
 * Read a top-level timings file and every child file (listed in `children`,
 * recursively, plus unlisted `<top>.<pid>.json` siblings). Returns null when
 * the top-level file does not exist (timings disabled or unsupported).
 */
export function loadTimings(topPath) {
  if (!existsSync(topPath)) return null;
  const top = readJson(topPath);
  if (top.schema !== 1) throw new Error(`${topPath}: unsupported timings schema ${top.schema}`);
  const children = [];
  const missing = [];
  const seen = new Set([resolve(topPath)]);
  const queue = (top.children ?? []).map((c) => [topPath, c]);
  // Unlisted siblings (e.g. a child that wrote its file after the parent's list was built).
  const prefix = `${basename(topPath)}.`;
  for (const name of readdirSync(dirname(topPath))) {
    if (name.startsWith(prefix) && /^\d+\.json$/.test(name.slice(prefix.length))) {
      queue.push([topPath, join(dirname(topPath), name)]);
    }
  }
  while (queue.length) {
    const [parent, child] = queue.shift();
    const path = resolveChild(parent, child);
    if (path === null) {
      missing.push(child);
      continue;
    }
    if (seen.has(resolve(path))) continue;
    seen.add(resolve(path));
    const doc = readJson(path);
    children.push(doc);
    for (const c of doc.children ?? []) queue.push([path, c]);
  }
  return { top, children, missing, merged: mergeTimings(top, children) };
}

/**
 * Merge a run's processes into one phase table. Phases sum across processes
 * (children run while the parent waits on them, so their phases are on the
 * critical path); `cpu_ns` sums where present. Wall, exec_to_main, and marks
 * come from the top-level process.
 */
export function mergeTimings(top, children = []) {
  const phases = {};
  const byProcess = [];
  for (const doc of [top, ...children]) {
    byProcess.push({ arm: doc.arm, pid: doc.pid, wall_ns: doc.wall_ns, phases: doc.phases ?? {} });
    for (const [name, p] of Object.entries(doc.phases ?? {})) {
      const acc = (phases[name] ??= { ns: 0, count: 0 });
      acc.ns += p.ns ?? 0;
      acc.count += p.count ?? 0;
      if (p.cpu_ns !== undefined) acc.cpu_ns = (acc.cpu_ns ?? 0) + p.cpu_ns;
    }
  }
  const phase_ns = Object.values(phases).reduce((a, p) => a + p.ns, 0);
  return {
    arm: top.arm,
    wall_ns: top.wall_ns,
    exec_to_main_ns: top.exec_to_main_ns ?? null,
    marks: top.marks ?? {},
    threads_used: top.threads_used,
    phases,
    coverage: top.wall_ns > 0 ? phase_ns / top.wall_ns : NaN,
    processes: byProcess,
  };
}

/** The single run in a hyperfine export (`--runs 1`). */
export function loadHyperfineRun(path) {
  const doc = readJson(path);
  const r = doc.results?.[0];
  if (!r) throw new Error(`${path}: no results`);
  const exit = r.exit_codes?.[0] ?? 0;
  return {
    wall_s: r.times?.[0] ?? r.mean,
    user_s: r.user,
    system_s: r.system,
    exit,
  };
}

/**
 * One size directory written by rounds.sh: meta.json plus
 * raw/<round>-<arm>.json and raw/<round>-<arm>.timings.json.
 */
export function loadSizeDir(dir) {
  const meta = readJson(join(dir, "meta.json"));
  const rawDir = join(dir, "raw");
  const runs = [];
  for (const name of readdirSync(rawDir).sort()) {
    const m = /^(\d+)-(.+)\.json$/.exec(name);
    if (!m || name.includes(".timings.")) continue;
    const [, round, arm] = m;
    const run = loadHyperfineRun(join(rawDir, name));
    const timings = loadTimings(join(rawDir, `${round}-${arm}.timings.json`));
    runs.push({ round: Number(round), arm, ...run, timings });
  }
  return { dir, meta, runs };
}

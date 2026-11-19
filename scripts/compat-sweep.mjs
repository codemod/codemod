#!/usr/bin/env node
// Compatibility sweep: run codemod packages' fixtures with two codemod CLIs
// (a released one and a new build) and report where they differ.
//
//   node scripts/compat-sweep.mjs --old /path/to/released/codemod \
//     --new target/release/codemod --corpus ~/src/codemods [--corpus ...] \
//     [--filter <text>] [--exclude <text>] [--jobs 8] [--timeout 180] [--limit 50] [--out .compat] \
//     [--upgrade] [--retry <earlier report.json>]
//
// For every package (a directory with codemod.yaml) and every fixture under
// its tests/ (a directory with input/, or an input.<ext> file), each CLI runs
// the package's workflow on its own copy of the input:
//
//   codemod workflow run -w <package> --target <copy> --no-interactive --allow-dirty
//
// and the sweep compares what the runs leave and say: the files, whether the
// run succeeded, the metrics, and the transforms' warnings. Each run is also
// checked against the fixture's expected/ files when there are some.
//
// With --package-tests, the sweep instead runs each package's own `test`
// script (package.json) in a copy of the package, once per CLI, with
// `pnpm dlx codemod@…` / `npx codemod` replaced by that CLI, and compares
// whether the scripts pass. That covers `jssg test`, `jssg run`, `jssg exec`
// and `workflow validate` as packages use them.
//
// With --retry, only the fixtures that differed in an earlier report run.
//
// With --upgrade, the new CLI also converts a copy of each package with
// `codemod workflow upgrade` and runs the fixtures again with the resulting
// workflow.ts; differences from its workflow.yaml run show as `upgraded:*`.
//
// Writes <out>/report.json, a summary in <out>/summary.md, and every run's
// output under <out>/runs/.

import { spawn } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { availableParallelism, tmpdir } from "node:os";
import { basename, extname, join, relative, resolve, sep } from "node:path";

const options = parseArgs(process.argv.slice(2));
const out = resolve(options.out ?? ".compat");
rmSync(join(out, "runs"), { recursive: true, force: true });
mkdirSync(join(out, "runs"), { recursive: true });

if (options.packageTests) {
  await sweepPackageTests();
  process.exit(0);
}

const retry = options.retry
  ? new Set(
      JSON.parse(readFileSync(options.retry, "utf8"))
        .filter((result) => result.differences.length)
        .map((result) => result.id),
    )
  : null;
const cases = discover(options.corpus, options.filter)
  .filter((item) => !options.exclude || !item.id.includes(options.exclude))
  .filter((item) => !retry || retry.has(item.id))
  .slice(0, options.limit ?? Infinity);
process.stderr.write(
  `${cases.length} fixtures in ${new Set(cases.map((c) => c.pkg)).size} packages\n`,
);

const results = [];
const upgrades = new Map();
const UPGRADED_NAME = ".compat-upgraded-workflow.ts";
const written = new Set();
const removeWritten = () => {
  for (const path of written) rmSync(path, { force: true });
};
process.on("exit", removeWritten);
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    removeWritten();
    process.exit(130);
  });
}

let done = 0;
await pool(
  cases,
  options.jobs ?? Math.max(2, Math.floor(availableParallelism() / 2)),
  async (item) => {
    const [oldRun, newRun] = await Promise.all([
      runCase(options.old, item, "old"),
      runCase(options.new, item, "new"),
    ]);
    const result = compare(item, oldRun, newRun);
    if (options.upgrade) {
      const upgraded = await upgradedPackage(item.pkg);
      if (upgraded.error) {
        result.differences.push({ kind: "upgraded:upgrade", error: upgraded.error });
      } else {
        const upgradedRun = await runCase(
          options.new,
          { ...item, workflow: upgraded.workflow },
          "upgraded",
          item.id,
        );
        for (const difference of compare(item, newRun, upgradedRun).differences) {
          result.differences.push({ ...difference, kind: `upgraded:${difference.kind}` });
        }
      }
    }
    results.push(result);
    done++;
    if (done % 25 === 0 || done === cases.length) {
      const differing = results.filter((r) => r.differences.length).length;
      process.stderr.write(`${done}/${cases.length} (${differing} differ)\n`);
    }
  },
);

results.sort((a, b) => a.id.localeCompare(b.id));
writeFileSync(join(out, "report.json"), `${JSON.stringify(results, null, 2)}\n`);
writeFileSync(join(out, "summary.md"), summarize(results));
process.stderr.write(
  `\n${summarize(results).split("\n## ")[0]}\nreport: ${join(out, "summary.md")}\n`,
);

// ------------------------------------------------------------ discovery ---

function discover(roots, filter) {
  const found = [];
  for (const root of roots) {
    walkPackages(resolve(root), (pkg) => {
      const tests = join(pkg, "tests");
      if (!isDir(tests)) return;
      for (const fixture of fixturesIn(tests)) {
        const id = `${relative(resolve(root, ".."), pkg)}#${relative(tests, fixture.dir) || "."}`;
        if (filter && !id.includes(filter)) continue;
        found.push({ id, pkg, ...fixture });
      }
    });
  }
  return found.sort((a, b) => a.id.localeCompare(b.id));
}

function walkPackages(dir, visit) {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return;
  }
  if (entries.some((entry) => entry.isFile() && entry.name === "codemod.yaml")) {
    visit(dir);
    return;
  }
  for (const entry of entries) {
    if (!entry.isDirectory() || ["node_modules", ".git", "target", "dist"].includes(entry.name)) {
      continue;
    }
    walkPackages(join(dir, entry.name), visit);
  }
}

/** Fixtures: directories with input/ (and maybe expected/), or input.<ext> files. */
function fixturesIn(dir, found = []) {
  if (isDir(join(dir, "input"))) {
    found.push({ dir, input: join(dir, "input"), expected: optional(join(dir, "expected")) });
    return found;
  }
  const entries = readdirSync(dir, { withFileTypes: true });
  const inputFile = entries.find(
    (e) => e.isFile() && basename(e.name, extname(e.name)) === "input",
  );
  if (inputFile) {
    const ext = extname(inputFile.name);
    found.push({
      dir,
      input: join(dir, inputFile.name),
      expected: optional(join(dir, `expected${ext}`)),
    });
    return found;
  }
  for (const entry of entries) {
    if (entry.isDirectory()) fixturesIn(join(dir, entry.name), found);
  }
  return found;
}

function optional(path) {
  return existsSync(path) ? path : null;
}

function isDir(path) {
  return existsSync(path) && statSync(path).isDirectory();
}

// -------------------------------------------------------------- running ---

/**
 * The package's workflow converted with `codemod workflow upgrade`, once per
 * package. It is written inside the package under a hidden name (removed when
 * the sweep ends), so its imports resolve as the YAML workflow's do.
 */
function upgradedPackage(pkg) {
  if (!upgrades.has(pkg)) {
    upgrades.set(
      pkg,
      (async () => {
        const run = await exec(options.new, ["workflow", "upgrade", "--dry-run", pkg], 60_000);
        if (run.code !== 0) return { error: run.output.trim().slice(0, 300) };
        const files = run.output.split(/^\/\/ (?=\/)/m).filter((part) => part.trim());
        // A package may hold more than one workflow; the fixtures run the one
        // codemod.yaml names.
        const main = manifestWorkflow(pkg)?.replace(/\.ya?ml$/, ".ts");
        const file =
          files.length === 1
            ? files[0]
            : files.find(
                (part) => main && part.slice(0, part.indexOf("\n")).trim() === join(pkg, main),
              );
        if (!file) return { error: `upgrade wrote ${files.length} workflows` };
        const workflow = join(pkg, UPGRADED_NAME);
        written.add(workflow);
        writeFileSync(workflow, file.slice(file.indexOf("\n") + 1));
        return { workflow };
      })(),
    );
  }
  return upgrades.get(pkg);
}

/** The workflow a package's codemod.yaml runs by default, if it names one. */
function manifestWorkflow(pkg) {
  try {
    const manifest = readFileSync(join(pkg, "codemod.yaml"), "utf8");
    const single = manifest.match(/^workflow:\s*["']?([^"'\s#]+)/m)?.[1];
    if (single) return single;
    // `workflows:` lists several; the default one, else the first.
    const entries = (manifest.split(/^workflows:/m)[1] ?? "").split(/^\s*-\s/m).slice(1);
    const path = (entry) => entry.match(/path:\s*["']?([^"'\s#]+)/)?.[1];
    return path(entries.find((entry) => /default:\s*true/.test(entry)) ?? entries[0] ?? "");
  } catch {
    return undefined;
  }
}

async function runCase(binary, item, which, recordId = item.id) {
  const work = mkdtempSync(join(tmpdir(), `compat-${which}-`));
  const target = join(work, "target");
  if (statSync(item.input).isFile()) {
    mkdirSync(target);
    cpSync(item.input, join(target, basename(item.input)));
  } else {
    cpSync(item.input, target, { recursive: true });
  }
  const started = Date.now();
  const run = await exec(
    binary,
    [
      "workflow",
      "run",
      "-w",
      item.workflow ?? item.pkg,
      "--target",
      target,
      "--no-interactive",
      "--allow-dirty",
    ],
    (options.timeout ?? 180) * 1000,
  );
  const files = readTree(target);
  // Each run has its own copy: name it the same in both outputs.
  const output = [realpathSync(target), target].reduce(
    (text, path) => text.split(path).join("<target>"),
    run.output,
  );
  rmSync(work, { recursive: true, force: true });
  const record = join(out, "runs", recordId.replace(/[/#]/g, "__"));
  mkdirSync(record, { recursive: true });
  writeFileSync(join(record, `${which}.txt`), output);
  return {
    code: run.code,
    timedOut: run.timedOut,
    ms: Date.now() - started,
    files,
    metrics: parseMetrics(output),
    warnings: parseWarnings(output),
  };
}

function exec(binary, args, timeout, cwd) {
  return new Promise((resolvePromise) => {
    const child = spawn(binary, args, {
      cwd,
      env: { ...process.env, NO_COLOR: "1", DISABLE_ANALYTICS: "true", CI: "1" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let output = "";
    child.stdout.on("data", (chunk) => (output += chunk));
    child.stderr.on("data", (chunk) => (output += chunk));
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, timeout);
    child.on("close", (code) => {
      clearTimeout(timer);
      resolvePromise({ code: timedOut ? null : code, timedOut, output });
    });
    child.on("error", (error) => {
      clearTimeout(timer);
      resolvePromise({ code: null, timedOut: false, output: String(error) });
    });
  });
}

function readTree(dir) {
  const tree = {};
  const walk = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile()) {
        tree[relative(dir, path).split(sep).join("/")] = readFileSync(path, "utf8").replace(
          /\r\n/g,
          "\n",
        );
      }
    }
  };
  if (existsSync(dir)) walk(dir);
  return tree;
}

// -------------------------------------------------------------- parsing ---

/**
 * The "Metrics:" block, as `{ metric: [entry, ...] }` with each entry's
 * dimensions sorted, since the CLIs print them in hash order.
 */
function parseMetrics(output) {
  const at = output.lastIndexOf("\nMetrics:\n");
  if (at === -1) return {};
  const metrics = {};
  let current = null;
  for (const line of output.slice(at + "\nMetrics:\n".length).split("\n")) {
    if (line.startsWith("Tip: ")) break;
    if (/^ {2}\S.*:$/.test(line)) {
      current = line.trim().slice(0, -1);
      metrics[current] = [];
    } else if (current && /^ {4}\S/.test(line)) {
      metrics[current].push(line.slice(4));
    } else if (current && metrics[current].length) {
      // A value with a newline in it continues on the next lines.
      metrics[current][metrics[current].length - 1] += `\n${line}`;
    }
  }
  for (const name of Object.keys(metrics)) {
    metrics[name] = metrics[name].map((entry) => canonicalEntry(entry.trimEnd())).sort();
  }
  return metrics;
}

function canonicalEntry(line) {
  const match = line.match(/^([\s\S]*): (\d+)$/);
  if (!match) return line;
  return `${match[1].split(", ").sort().join(", ")}: ${match[2]}`;
}

/** The transforms' `[warn]` lines, sorted. */
function parseWarnings(output) {
  return output
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.startsWith("[warn] "))
    .sort();
}

// ------------------------------------------------------------ comparing ---

function compare(item, oldRun, newRun) {
  const differences = [];
  const oldOk = oldRun.code === 0;
  const newOk = newRun.code === 0;
  if (oldRun.timedOut !== newRun.timedOut) {
    differences.push({ kind: "timeout", old: oldRun.timedOut, new: newRun.timedOut });
  } else if (oldOk !== newOk) {
    differences.push({ kind: "status", old: oldRun.code, new: newRun.code });
  }
  const changed = changedFiles(oldRun.files, newRun.files);
  if (changed.length) differences.push({ kind: "files", paths: changed });
  if (canonical(oldRun.metrics) !== canonical(newRun.metrics)) {
    differences.push({
      kind: "metrics",
      detail: metricsDifference(oldRun.metrics, newRun.metrics),
    });
  }
  if (JSON.stringify(oldRun.warnings) !== JSON.stringify(newRun.warnings)) {
    differences.push({
      kind: "warnings",
      old: oldRun.warnings.length,
      new: newRun.warnings.length,
    });
  }
  const expected = item.expected ? expectedTree(item) : null;
  return {
    id: item.id,
    pkg: item.pkg,
    differences,
    old: {
      code: oldRun.code,
      ms: oldRun.ms,
      matchesExpected: expected && matches(oldRun.files, expected),
    },
    new: {
      code: newRun.code,
      ms: newRun.ms,
      matchesExpected: expected && matches(newRun.files, expected),
    },
  };
}

/** JSON with object keys sorted, so key order does not count as a difference. */
function canonical(value) {
  return JSON.stringify(value, (_key, item) =>
    item && typeof item === "object" && !Array.isArray(item)
      ? Object.fromEntries(Object.entries(item).sort(([a], [b]) => a.localeCompare(b)))
      : item,
  );
}

function changedFiles(a, b) {
  return [...new Set([...Object.keys(a), ...Object.keys(b)])]
    .filter((path) => a[path] !== b[path])
    .sort();
}

function expectedTree(item) {
  if (statSync(item.expected).isFile()) {
    return { [basename(item.input)]: readFileSync(item.expected, "utf8").replace(/\r\n/g, "\n") };
  }
  return readTree(item.expected);
}

function matches(files, expected) {
  // Ignore files the fixtures keep beside the code (metrics snapshots).
  const keep = (tree) =>
    Object.fromEntries(Object.entries(tree).filter(([path]) => !path.endsWith("metrics.json")));
  return changedFiles(keep(files), keep(expected)).length === 0;
}

function metricsDifference(a, b) {
  const names = [...new Set([...Object.keys(a), ...Object.keys(b)])].sort();
  const lines = [];
  for (const name of names) {
    const left = new Set(a[name] ?? []);
    const right = new Set(b[name] ?? []);
    const missing = [...left].filter((entry) => !right.has(entry));
    const extra = [...right].filter((entry) => !left.has(entry));
    if (missing.length || extra.length) {
      lines.push(`${name}: ${missing.length} only in old, ${extra.length} only in new`);
    }
  }
  return lines.slice(0, 10);
}

// ------------------------------------------------------------ reporting ---

function summarize(results) {
  const differing = results.filter((r) => r.differences.length);
  const byKind = {};
  for (const result of differing) {
    for (const difference of result.differences) {
      (byKind[difference.kind] ??= []).push({ result, difference });
    }
  }
  const expectedCases = results.filter((r) => r.old.matchesExpected !== null);
  const count = (which) => expectedCases.filter((r) => r[which].matchesExpected).length;
  const regressions = expectedCases.filter((r) => r.old.matchesExpected && !r.new.matchesExpected);
  const time = (which) => results.reduce((sum, r) => sum + r[which].ms, 0) / 1000;

  let text = `# Compatibility sweep\n\n`;
  text += `- Fixtures: ${results.length} in ${new Set(results.map((r) => r.pkg)).size} packages\n`;
  text += `- Same outcome: ${results.length - differing.length}; differ: ${differing.length}\n`;
  for (const [kind, items] of Object.entries(byKind)) text += `  - ${kind}: ${items.length}\n`;
  text += `- Matches expected/: old ${count("old")}/${expectedCases.length}, new ${count("new")}/${expectedCases.length}\n`;
  text += `- Passed on old, fails on new: ${regressions.length}\n`;
  text += `- Total run time: old ${time("old").toFixed(0)}s, new ${time("new").toFixed(0)}s\n`;

  if (regressions.length) {
    text += `\n## Passed on old, fails on new\n\n`;
    for (const r of regressions) text += `- ${r.id}\n`;
  }
  for (const [kind, items] of Object.entries(byKind)) {
    text += `\n## ${kind}\n\n`;
    for (const { result, difference } of items.slice(0, 200)) {
      const { kind: _, ...detail } = difference;
      text += `- ${result.id}: ${JSON.stringify(detail).slice(0, 300)}\n`;
    }
  }
  return text;
}

// -------------------------------------------------------- package tests ---

/** `pnpm dlx codemod@latest`, `npx codemod` and the like. */
function launcher() {
  return /(?:pnpm dlx|pnpx|npx(?: -y| --yes)?|bunx|yarn dlx)\s+codemod(?:@[\w.-]+)?(?=\s|$)/g;
}

async function sweepPackageTests() {
  const packages = [];
  for (const root of options.corpus) {
    walkPackages(resolve(root), (pkg) => {
      let script;
      try {
        script = JSON.parse(readFileSync(join(pkg, "package.json"), "utf8")).scripts?.test;
      } catch {
        return;
      }
      if (!script || !launcher().test(script)) return;
      const id = relative(resolve(root, ".."), pkg);
      if (options.filter && !id.includes(options.filter)) return;
      packages.push({ id, pkg, script });
    });
  }
  packages.sort((a, b) => a.id.localeCompare(b.id));
  const selected = packages.slice(0, options.limit ?? Infinity);
  process.stderr.write(`${selected.length} package test scripts\n`);

  const results = [];
  let finished = 0;
  await pool(
    selected,
    options.jobs ?? Math.max(2, Math.floor(availableParallelism() / 2)),
    async (item) => {
      // One after the other: some scripts write to fixed paths under /tmp.
      const oldRun = await runPackageTest(options.old, item, "old");
      const newRun = await runPackageTest(options.new, item, "new");
      results.push({ id: item.id, old: oldRun, new: newRun });
      finished++;
      if (finished % 25 === 0 || finished === selected.length) {
        const broken = results.filter((r) => r.old.ok && !r.new.ok).length;
        process.stderr.write(
          `${finished}/${selected.length} (${broken} pass on old, fail on new)\n`,
        );
      }
    },
  );
  results.sort((a, b) => a.id.localeCompare(b.id));
  writeFileSync(join(out, "package-tests.json"), `${JSON.stringify(results, null, 2)}\n`);

  const broken = results.filter((r) => r.old.ok && !r.new.ok);
  const fixed = results.filter((r) => !r.old.ok && r.new.ok);
  const bothFail = results.filter((r) => !r.old.ok && !r.new.ok);
  let text = "# Package test scripts\n\n";
  text += `- Packages: ${results.length}\n`;
  text += `- Pass on both: ${results.filter((r) => r.old.ok && r.new.ok).length}\n`;
  text += `- Pass on old, fail on new: ${broken.length}\n`;
  text += `- Fail on old, pass on new: ${fixed.length}\n`;
  text += `- Fail on both: ${bothFail.length}\n`;
  for (const [title, list] of [
    ["Pass on old, fail on new", broken],
    ["Fail on old, pass on new", fixed],
    ["Fail on both", bothFail],
  ]) {
    if (!list.length) continue;
    text += `\n## ${title}\n\n`;
    for (const r of list.slice(0, 300)) text += `- ${r.id}: ${r.new.tail}\n`;
  }
  writeFileSync(join(out, "package-tests.md"), text);
  process.stderr.write(`\n${text.split("\n## ")[0]}\nreport: ${join(out, "package-tests.md")}\n`);
}

async function runPackageTest(binary, item, which) {
  const work = mkdtempSync(join(tmpdir(), `compat-pkg-${which}-`));
  const copy = join(work, basename(item.pkg));
  cpSync(item.pkg, copy, {
    recursive: true,
    filter: (path) => !/[/\\](node_modules|\.git)$/.test(path),
  });
  const script = item.script.replace(launcher(), JSON.stringify(binary));
  const started = Date.now();
  const run = await exec("sh", ["-c", script], (options.timeout ?? 300) * 1000, copy);
  rmSync(work, { recursive: true, force: true });
  const output = run.output.split(copy).join("<package>");
  const record = join(out, "runs", item.id.replace(/[/#]/g, "__"));
  mkdirSync(record, { recursive: true });
  writeFileSync(join(record, `test-${which}.txt`), output);
  const lines = output
    .trim()
    .split("\n")
    .filter((line) => line.trim());
  return {
    ok: run.code === 0,
    code: run.code,
    timedOut: run.timedOut,
    ms: Date.now() - started,
    tail: lines.slice(-2).join(" | ").slice(0, 300),
  };
}

// ---------------------------------------------------------------- misc ---

async function pool(items, size, work) {
  let next = 0;
  await Promise.all(
    Array.from({ length: Math.min(size, items.length) }, async () => {
      while (next < items.length) await work(items[next++]);
    }),
  );
}

function parseArgs(args) {
  const parsed = { corpus: [] };
  for (let i = 0; i < args.length; i++) {
    const key = args[i].replace(/^--/, "");
    const value = args[++i];
    if (key === "corpus") parsed.corpus.push(value);
    else if (key === "upgrade" || key === "package-tests") {
      parsed[key === "upgrade" ? "upgrade" : "packageTests"] = true;
      i--;
    } else if (["jobs", "timeout", "limit"].includes(key)) parsed[key] = Number(value);
    else parsed[key] = value;
  }
  if (!parsed.old || !parsed.new || !parsed.corpus.length) {
    process.stderr.write(
      "usage: node scripts/compat-sweep.mjs --old <codemod> --new <codemod> --corpus <dir> [--corpus <dir>] [--filter <text>] [--exclude <text>] [--jobs N] [--timeout s] [--limit N] [--out dir] [--upgrade | --package-tests] [--retry report.json]\n",
    );
    process.exit(2);
  }
  parsed.old = resolve(parsed.old);
  parsed.new = resolve(parsed.new);
  return parsed;
}
